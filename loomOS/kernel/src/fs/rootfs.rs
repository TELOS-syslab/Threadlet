// SPDX-License-Identifier: MPL-2.0

use alloc::format;
use cpio_decoder::{CpioDecoder, FileType};
use lending_iterator::LendingIterator;
use libflate::gzip::Decoder as GZipDecoder;
use spin::Once;
use core2::io::{Write as IoWrite, Result as IoWriteResult};

use super::{
    fs_resolver::{FsPath, FsResolver},
    path::MountNode,
    procfs::{self, ProcFS},
    ramfs::RamFS,
    utils::{FileSystem, InodeMode, InodeType},
};
use crate::prelude::*;

/// A writer wrapper that reports incremental progress while writing data.
struct ProgressWriter<W: IoWrite> {
    inner: W,
    file_name: alloc::string::String,
    total: usize,
    written: usize,
    next_report: usize,
    step: usize,
}

impl<W: IoWrite> ProgressWriter<W> {
    fn new(inner: W, file_name: &str, total: usize, step: usize) -> Self {
        Self {
            inner,
            file_name: alloc::string::String::from(file_name),
            total,
            written: 0,
            next_report: step.min(total),
            step: step.max(1),
        }
    }
}

impl<W: IoWrite> IoWrite for ProgressWriter<W> {
    fn write(&mut self, buf: &[u8]) -> IoWriteResult<usize> {
        // Debug prints commented out per test plan
        // ostd::early_println!(
        //     "[rootfs] pw.enter file='{}' req={} written={}",
        //     self.file_name,
        //     buf.len(),
        //     self.written
        // );
        let n = self.inner.write(buf)?;
        // ostd::early_println!(
        //     "[rootfs] pw.ret   file='{}' n={} new_written={}",
        //     self.file_name,
        //     n,
        //     self.written.saturating_add(n)
        // );
        self.written = self.written.saturating_add(n);
        if self.written >= self.next_report {

        }
        ostd::early_println!(
            "[rootfs] Large file: '{}' progress {}/{} bytes",
            self.file_name,
            self.written,
            self.total
        );
        self.next_report = (self.next_report + self.step).min(self.total);
        Ok(n)
    }

    fn flush(&mut self) -> IoWriteResult<()> {
        self.inner.flush()
    }
}

/// Unpack and prepare the rootfs from the initramfs CPIO buffer.
pub fn init(initramfs_buf: &[u8]) -> Result<()> {
    init_root_mount();
    procfs::init();

    ostd::early_println!("[kernel] unpacking the initramfs.cpio.gz to rootfs ...");
    let fs = FsResolver::new();
    // Lightweight progress counters for unpacking
    let mut n_entries: usize = 0;
    let mut n_files: usize = 0;
    let mut n_dirs: usize = 0;
    let mut n_links: usize = 0;
    let mut bytes_files: usize = 0;
    // Debug: print incoming initramfs buffer basic info and magic
    {
        let magic = initramfs_buf.get(0..4).unwrap_or(&[]);
        ostd::early_println!(
            "[rootfs] initramfs_buf: len={} magic={:02x?}",
            initramfs_buf.len(),
            magic
        );
    }

    // Materialize gzip decoder with extra diagnostics on failure.
    let gz = match GZipDecoder::new(initramfs_buf) {
        Ok(decoder) => decoder,
        Err(_e) => {
            let magic = initramfs_buf.get(0..8).unwrap_or(&[]);
            ostd::early_println!(
                "[rootfs] invalid gzip buffer: len={} magic-head={:02x?}",
                initramfs_buf.len(),
                magic
            );
            return Err(Error::with_message(Errno::EINVAL, "invalid gzip buffer"));
        }
    };
    let mut decoder = CpioDecoder::new(gz);

    ostd::early_println!("[rootfs] checkpoint 1");

    loop {
        let Some(entry_result) = decoder.next() else {
            break;
        };

        let mut entry = match entry_result {
            Ok(e) => e,
            Err(e) => {
                ostd::early_println!(
                    "[rootfs] decoder.next() error at entry#{}: {:?}",
                    n_entries,
                    e
                );
                return Err(e.into());
            }
        };
        n_entries += 1;

        // Make sure the name is a relative path, and is not end with "/".
        let entry_name_owned: String = entry
            .name()
            .trim_start_matches('/')
            .trim_end_matches('/')
            .to_string();
        let entry_name: &str = entry_name_owned.as_str();
        if entry_name.is_empty() {
            ostd::early_println!("[rootfs] invalid empty entry name at #{}", n_entries);
            return_errno_with_message!(Errno::EINVAL, "invalid entry name");
        }
        if entry_name == "." {
            continue;
        }

        // Here we assume that the directory referred by "prefix" must has been created.
        // The basis of this assumption is：
        // The mkinitramfs script uses `find` command to ensure that the entries are
        // sorted that a directory always appears before its child directories and files.
        let (parent, name) = if let Some((prefix, last)) = entry_name.rsplit_once('/') {
            let path = match FsPath::try_from(prefix) {
                Ok(p) => p,
                Err(e) => {
                    ostd::early_println!(
                        "[rootfs] FsPath::try_from failed for prefix='{}': {:?}",
                        prefix, e
                    );
                    return Err(e);
                }
            };
            match fs.lookup(&path) {
                Ok(p) => (p, last),
                Err(e) => {
                    ostd::early_println!(
                        "[rootfs] parent lookup failed for '{}' (prefix='{}'): {:?}",
                        entry_name, prefix, e
                    );
                    return Err(e);
                }
            }
        } else {
            (fs.root().clone(), entry_name)
        };
        ostd::early_println!("[rootfs] checkpoint 2: entry#{} '{}'", n_entries, entry_name);

        let metadata = entry.metadata().clone();
        let mode = InodeMode::from_bits_truncate(metadata.permission_mode());
        match metadata.file_type() {
            FileType::File => {
                n_files += 1;
                let size = metadata.size() as usize;
                bytes_files = bytes_files.saturating_add(size);
                if size >= (1 << 20) {
                    // Print large file progress (>= 1MiB)
                    ostd::early_println!(
                        "[rootfs] large file: '{}' size={} bytes - start",
                        entry_name, size
                    );
                }
                let dentry = match parent.new_fs_child(name, InodeType::File, mode) {
                    Ok(d) => d,
                    Err(e) => {
                        ostd::early_println!(
                            "[rootfs] new_fs_child(File) failed for '{}': {:?}",
                            entry_name, e
                        );
                        return Err(e);
                    }
                };
                if size >= (1 << 20) {
                    // For large files, wrap writer to report incremental progress every 256 KiB.
                    let writer = dentry.inode().writer(0);
                    let pw = ProgressWriter::new(writer, entry_name, size, 256 * 1024);
                    if let Err(e) = entry.read_all(pw) {
                        ostd::early_println!(
                            "[rootfs] read_all(File) failed for '{}' size={}: {:?}",
                            entry_name, size, e
                        );
                        return Err(e.into());
                    }
                } else {
                    if let Err(e) = entry.read_all(dentry.inode().writer(0)) {
                        ostd::early_println!(
                            "[rootfs] read_all(File) failed for '{}' size={}: {:?}",
                            entry_name, size, e
                        );
                        return Err(e.into());
                    }
                }
                if size >= (1 << 20) {
                    ostd::early_println!(
                        "[rootfs] large file: '{}' — done",
                        entry_name
                    );
                }
            }
            FileType::Dir => {
                n_dirs += 1;
                if let Err(e) = parent.new_fs_child(name, InodeType::Dir, mode) {
                    ostd::early_println!(
                        "[rootfs] new_fs_child(Dir) failed for '{}': {:?}",
                        entry_name, e
                    );
                    return Err(e);
                }
            }
            FileType::Link => {
                n_links += 1;
                let dentry = match parent.new_fs_child(name, InodeType::SymLink, mode) {
                    Ok(d) => d,
                    Err(e) => {
                        ostd::early_println!(
                            "[rootfs] new_fs_child(Link) failed for '{}': {:?}",
                            entry_name, e
                        );
                        return Err(e);
                    }
                };
                let link_content = {
                    let mut link_data: Vec<u8> = Vec::new();
                    if let Err(e) = entry.read_all(&mut link_data) {
                        ostd::early_println!(
                            "[rootfs] read_all(Link) failed for '{}': {:?}",
                            entry_name, e
                        );
                        return Err(e.into());
                    }
                    match core::str::from_utf8(&link_data) {
                        Ok(s) => s.to_string(),
                        Err(e) => {
                            ostd::early_println!(
                                "[rootfs] link content utf8 error for '{}': {:?}",
                                entry_name, e
                            );
                            return_errno_with_message!(Errno::EINVAL, "invalid symlink content");
                        }
                    }
                };
                if let Err(e) = dentry.inode().write_link(&link_content) {
                    ostd::early_println!(
                        "[rootfs] write_link failed for '{}': {:?}",
                        entry_name, e
                    );
                    return Err(e);
                }
            }
            type_ => {
                // Extra diagnostics for unsupported entries
                ostd::early_println!(
                    "[rootfs] unsupported file type {:?} for entry '{}': aborting",
                    type_,
                    entry_name
                );
                panic!("unsupported file type = {:?} in initramfs", type_);
            }
        }

        // Periodic progress log every 128 entries
        if (n_entries & 0x7f) == 0 {
            ostd::early_println!(
                "[rootfs] progress: entries={} files={} dirs={} links={} bytes_total_files={}",
                n_entries, n_files, n_dirs, n_links, bytes_files
            );
        }
    }
    // Mount ProcFS
    ostd::early_println!("[rootfs] mounting /proc");
    let proc_dentry = match FsPath::try_from("/proc") {
        Ok(p) => fs.lookup(&p)?,
        Err(e) => {
            ostd::early_println!("[rootfs] FsPath::try_from('/proc') failed: {:?}", e);
            return Err(e);
        }
    };
    if let Err(e) = proc_dentry.mount(ProcFS::new()) {
        ostd::early_println!("[rootfs] mount /proc failed: {:?}", e);
        return Err(e);
    }
    // Mount DevFS
    ostd::early_println!("[rootfs] mounting /dev");
    let dev_dentry = match FsPath::try_from("/dev") {
        Ok(p) => fs.lookup(&p)?,
        Err(e) => {
            ostd::early_println!("[rootfs] FsPath::try_from('/dev') failed: {:?}", e);
            return Err(e);
        }
    };
    if let Err(e) = dev_dentry.mount(RamFS::new()) {
        ostd::early_println!("[rootfs] mount /dev failed: {:?}", e);
        return Err(e);
    }

    // Install builtin assets like busybox if provided by the boot layer.
    install_builtin_busybox()?;

    ostd::early_println!("[kernel] rootfs is ready");

    Ok(())
}

pub fn mount_fs_at(fs: Arc<dyn FileSystem>, fs_path: &FsPath) -> Result<()> {
    let target_dentry = FsResolver::new().lookup(fs_path)?;
    target_dentry.mount(fs)?;
    Ok(())
}

static ROOT_MOUNT: Once<Arc<MountNode>> = Once::new();

pub fn init_root_mount() {
    ROOT_MOUNT.call_once(|| -> Arc<MountNode> {
        let rootfs = RamFS::new();
        MountNode::new_root(rootfs)
    });
}

pub fn root_mount() -> &'static Arc<MountNode> {
    ROOT_MOUNT.get().unwrap()
}

/// Install builtin busybox payload embedded into the kernel image.
fn install_builtin_busybox() -> Result<()> {
    use crate::fs::utils::{Inode, InodeMode, InodeType};

    let busy = ostd::boot::builtin_busybox();
    if busy.is_empty() {
        return Ok(());
    }

    let fs = FsResolver::new();

    // Ensure necessary directories exist.
    let ensure_dir = |path: &str| -> Result<()> {
        if let Ok(_) = FsPath::try_from(path).and_then(|p| fs.lookup(&p)) {
            return Ok(());
        }
        // Create recursively from root.
        let mut cur = fs.root().clone();
        for comp in path.trim_start_matches('/').split('/') {
            if comp.is_empty() { continue; }
            let full = format!("{}/{}", cur.abs_path(), comp);
            let next = match FsPath::try_from(full.as_str()).and_then(|p| fs.lookup(&p)) {
                Ok(d) => d,
                Err(_) => cur.new_fs_child(comp, InodeType::Dir, InodeMode::from_bits_truncate(0o755))?,
            };
            cur = next;
        }
        Ok(())
    };

    ensure_dir("/usr/bin")?;
    ensure_dir("/bin")?;

    let usr_bin = fs.lookup(&FsPath::try_from("/usr/bin")?)?;
    let busybox_path = FsPath::try_from("/usr/bin/busybox")?;
    if fs.lookup(&busybox_path).is_err() {
        let dentry = usr_bin
            .new_fs_child("busybox", InodeType::File, InodeMode::from_bits_truncate(0o755))?;
        dentry.inode().write_bytes_at(0, busy)?;
        ostd::early_println!(
            "[rootfs] installed builtin busybox: size={} bytes",
            busy.len()
        );
    }

    // Create /bin/busybox symlink if missing
    let bin_dir = fs.lookup(&FsPath::try_from("/bin")?)?;
    let bin_busybox = FsPath::try_from("/bin/busybox")?;
    if fs.lookup(&bin_busybox).is_err() {
        let dentry = bin_dir
            .new_fs_child("busybox", InodeType::SymLink, InodeMode::from_bits_truncate(0o777))?;
        dentry.inode().write_link("../usr/bin/busybox")?;
    }

    Ok(())
}
