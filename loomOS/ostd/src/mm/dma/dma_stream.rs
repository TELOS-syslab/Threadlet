// SPDX-License-Identifier: MPL-2.0

use alloc::sync::Arc;
use core::ops::Range;

use cfg_if::cfg_if;

use super::{check_and_insert_dma_mapping, remove_dma_mapping, DmaError, HasDaddr};
use crate::{
    arch::iommu,
    error::Error,
    mm::{
        dma::{dma_type, Daddr, DmaType},
        HasPaddr, Infallible, Paddr, USegment, UntypedMem, VmIo, VmReader, VmWriter, PAGE_SIZE,
    },
};
#[cfg(target_arch = "riscv64")]
use crate::mm::kspace::paddr_to_vaddr;
#[cfg(target_arch = "riscv64")]
use core::arch::asm;

cfg_if! {
    if #[cfg(all(target_arch = "x86_64", feature = "cvm_guest"))] {
        use ::tdx_guest::tdx_is_enabled;
        use crate::arch::tdx_guest;
    }
}

/// A streaming DMA mapping. Users must synchronize data
/// before reading or after writing to ensure consistency.
///
/// The mapping is automatically destroyed when this object
/// is dropped.
#[derive(Debug, Clone)]
pub struct DmaStream {
    inner: Arc<DmaStreamInner>,
}

#[derive(Debug)]
struct DmaStreamInner {
    segment: USegment,
    start_daddr: Daddr,
    #[doc = " TODO: remove this field when on x86."]
    #[allow(unused)]
    is_cache_coherent: bool,
    direction: DmaDirection,
}

/// `DmaDirection` limits the data flow direction of [`DmaStream`] and
/// prevents users from reading and writing to [`DmaStream`] unexpectedly.
#[derive(Debug, PartialEq, Clone, Copy)]
pub enum DmaDirection {
    /// Data flows to the device
    ToDevice,
    /// Data flows from the device
    FromDevice,
    /// Data flows both from and to the device
    Bidirectional,
}

impl DmaStream {
    /// Establishes DMA stream mapping for a given [`USegment`].
    ///
    /// The method fails if the segment already belongs to a DMA mapping.
    pub fn map(
        segment: USegment,
        direction: DmaDirection,
        is_cache_coherent: bool,
    ) -> Result<Self, DmaError> {
        let frame_count = segment.size() / PAGE_SIZE;
        let start_paddr = segment.start_paddr();
        if !check_and_insert_dma_mapping(start_paddr, frame_count) {
            return Err(DmaError::AlreadyMapped);
        }
        // Ensure that the addresses used later will not overflow
        start_paddr.checked_add(frame_count * PAGE_SIZE).unwrap();
        let start_daddr = match dma_type() {
            DmaType::Direct => start_paddr as Daddr,
            DmaType::Iommu => {
                for i in 0..frame_count {
                    let paddr = start_paddr + (i * PAGE_SIZE);
                    // SAFETY: the `paddr` is restricted by the `start_paddr` and `frame_count` of the `segment`.
                    unsafe {
                        iommu::map(paddr as Daddr, paddr).unwrap();
                    }
                }
                start_paddr as Daddr
            }
        };

        Ok(Self {
            inner: Arc::new(DmaStreamInner {
                segment,
                start_daddr,
                is_cache_coherent,
                direction,
            }),
        })
    }

    /// Gets the underlying [`USegment`].
    ///
    /// Usually, the CPU side should not access the memory
    /// after the DMA mapping is established because
    /// there is a chance that the device is updating
    /// the memory. Do this at your own risk.
    pub fn segment(&self) -> &USegment {
        &self.inner.segment
    }

    /// Returns the number of frames.
    pub fn nframes(&self) -> usize {
        self.inner.segment.size() / PAGE_SIZE
    }

    /// Returns the number of bytes.
    pub fn nbytes(&self) -> usize {
        self.inner.segment.size()
    }

    /// Returns the DMA direction.
    pub fn direction(&self) -> DmaDirection {
        self.inner.direction
    }

    /// Synchronizes the streaming DMA mapping with the device.
    ///
    /// This method should be called under one of the two conditions:
    /// 1. The data of the stream DMA mapping has been updated by the device side.
    ///    The CPU side needs to call the `sync` method before reading data (e.g., using [`read_bytes`]).
    /// 2. The data of the stream DMA mapping has been updated by the CPU side
    ///    (e.g., using [`write_bytes`]).
    ///    Before the CPU side notifies the device side to read, it must call the `sync` method first.
    ///
    /// [`read_bytes`]: Self::read_bytes
    /// [`write_bytes`]: Self::write_bytes
    pub fn sync(&self, byte_range: Range<usize>) -> Result<(), Error> {
        // Fast path: cache-coherent device or zero-length
        if self.inner.is_cache_coherent || byte_range.start >= byte_range.end {
            return Ok(());
        }

        cfg_if! {
            if #[cfg(target_arch = "riscv64")] {
                // Use RISC-V CBO operations (Zicbo*) to maintain cache lines.
                // - ToDevice: clean (writeback) lines before device reads.
                // - FromDevice: invalidate lines before CPU reads.
                // - Bidirectional: flush (clean+inval) to be conservative.
                // Note: We assume a 64-byte cache line if block-size is unknown.
                const CBO_LINE_SIZE: usize = 64;

                let start_paddr = self.inner.segment.start_paddr();
                let start_va = paddr_to_vaddr(start_paddr + byte_range.start);
                let end_va = paddr_to_vaddr(start_paddr + byte_range.end);
                let mut cur = start_va & !(CBO_LINE_SIZE - 1);
                let end_aligned = (end_va + (CBO_LINE_SIZE - 1)) & !(CBO_LINE_SIZE - 1);

                // Prefer Zicbo* when the target enables the ISA feature at compile time.
                // Otherwise fall back to a conservative fence which is sufficient for QEMU
                // (no DCache modeling) and won't require assembler Zicbo support.
                #[cfg(target_feature = "zicbom")]
                unsafe {
                    match self.inner.direction {
                        DmaDirection::ToDevice => {
                            while cur < end_aligned {
                                asm!(
                                    "cbo.clean 0({addr})",
                                    addr = in(reg) cur,
                                    options(nostack)
                                );
                                cur += CBO_LINE_SIZE;
                            }
                        }
                        DmaDirection::FromDevice => {
                            while cur < end_aligned {
                                asm!(
                                    "cbo.inval 0({addr})",
                                    addr = in(reg) cur,
                                    options(nostack)
                                );
                                cur += CBO_LINE_SIZE;
                            }
                        }
                        DmaDirection::Bidirectional => {
                            while cur < end_aligned {
                                asm!(
                                    "cbo.flush 0({addr})",
                                    addr = in(reg) cur,
                                    options(nostack)
                                );
                                cur += CBO_LINE_SIZE;
                            }
                        }
                    }
                    Ok(())
                }

                #[cfg(not(target_feature = "zicbom"))]
                unsafe {
                    // Fallback: full fence for ordering; does not clean/invalidate caches
                    // but sufficient for QEMU where DCache is not modeled.
                    asm!("fence rw, rw", options(nostack));
                    Ok(())
                }
            } else {
                // On non-RISC-V targets, no explicit cache maintenance is needed here.
                let _ = byte_range;
                Ok(())
            }
        }
    }
}

impl HasDaddr for DmaStream {
    fn daddr(&self) -> Daddr {
        self.inner.start_daddr
    }
}

impl Drop for DmaStreamInner {
    fn drop(&mut self) {
        let frame_count = self.segment.size() / PAGE_SIZE;
        let start_paddr = self.segment.start_paddr();
        // Ensure that the addresses used later will not overflow
        start_paddr.checked_add(frame_count * PAGE_SIZE).unwrap();
        match dma_type() {
            DmaType::Direct => {}
            DmaType::Iommu => {
                for i in 0..frame_count {
                    let paddr = start_paddr + (i * PAGE_SIZE);
                    iommu::unmap(paddr).unwrap();
                }
            }
        }
        remove_dma_mapping(start_paddr, frame_count);
    }
}

impl VmIo for DmaStream {
    /// Reads data into the buffer.
    fn read(&self, offset: usize, writer: &mut VmWriter) -> Result<(), Error> {
        if self.inner.direction == DmaDirection::ToDevice {
            return Err(Error::AccessDenied);
        }
        self.inner.segment.read(offset, writer)
    }

    /// Writes data from the buffer.
    fn write(&self, offset: usize, reader: &mut VmReader) -> Result<(), Error> {
        if self.inner.direction == DmaDirection::FromDevice {
            return Err(Error::AccessDenied);
        }
        self.inner.segment.write(offset, reader)
    }
}

impl<'a> DmaStream {
    /// Returns a reader to read data from it.
    pub fn reader(&'a self) -> Result<VmReader<'a, Infallible>, Error> {
        if self.inner.direction == DmaDirection::ToDevice {
            return Err(Error::AccessDenied);
        }
        Ok(self.inner.segment.reader())
    }

    /// Returns a writer to write data into it.
    pub fn writer(&'a self) -> Result<VmWriter<'a, Infallible>, Error> {
        if self.inner.direction == DmaDirection::FromDevice {
            return Err(Error::AccessDenied);
        }
        Ok(self.inner.segment.writer())
    }
}

impl HasPaddr for DmaStream {
    fn paddr(&self) -> Paddr {
        self.inner.segment.start_paddr()
    }
}

impl AsRef<DmaStream> for DmaStream {
    fn as_ref(&self) -> &DmaStream {
        self
    }
}

/// A slice of streaming DMA mapping.
#[derive(Debug)]
pub struct DmaStreamSlice<Dma> {
    stream: Dma,
    offset: usize,
    len: usize,
}

impl<Dma: AsRef<DmaStream>> DmaStreamSlice<Dma> {
    /// Constructs a `DmaStreamSlice` from the [`DmaStream`].
    ///
    /// # Panics
    ///
    /// If the `offset` is greater than or equal to the length of the stream,
    /// this method will panic.
    /// If the `offset + len` is greater than the length of the stream,
    /// this method will panic.
    pub fn new(stream: Dma, offset: usize, len: usize) -> Self {
        assert!(offset < stream.as_ref().nbytes());
        assert!(offset + len <= stream.as_ref().nbytes());

        Self {
            stream,
            offset,
            len,
        }
    }

    /// Returns the underlying `DmaStream`.
    pub fn stream(&self) -> &DmaStream {
        self.stream.as_ref()
    }

    /// Returns the offset of the slice.
    pub fn offset(&self) -> usize {
        self.offset
    }

    /// Returns the number of bytes.
    pub fn nbytes(&self) -> usize {
        self.len
    }

    /// Synchronizes the slice of streaming DMA mapping with the device.
    pub fn sync(&self) -> Result<(), Error> {
        self.stream
            .as_ref()
            .sync(self.offset..self.offset + self.len)
    }

    /// Returns a reader to read data from it.
    pub fn reader(&self) -> Result<VmReader<Infallible>, Error> {
        let stream_reader = self
            .stream
            .as_ref()
            .reader()?
            .skip(self.offset)
            .limit(self.len);
        Ok(stream_reader)
    }

    /// Returns a writer to write data into it.
    pub fn writer(&self) -> Result<VmWriter<Infallible>, Error> {
        let stream_writer = self
            .stream
            .as_ref()
            .writer()?
            .skip(self.offset)
            .limit(self.len);
        Ok(stream_writer)
    }
}

impl<Dma: AsRef<DmaStream> + Send + Sync> VmIo for DmaStreamSlice<Dma> {
    fn read(&self, offset: usize, writer: &mut VmWriter) -> Result<(), Error> {
        if writer.avail() + offset > self.len {
            return Err(Error::InvalidArgs);
        }
        self.stream.as_ref().read(self.offset + offset, writer)
    }

    fn write(&self, offset: usize, reader: &mut VmReader) -> Result<(), Error> {
        if reader.remain() + offset > self.len {
            return Err(Error::InvalidArgs);
        }
        self.stream.as_ref().write(self.offset + offset, reader)
    }
}

impl<Dma: AsRef<DmaStream>> HasDaddr for DmaStreamSlice<Dma> {
    fn daddr(&self) -> Daddr {
        self.stream.as_ref().daddr() + self.offset
    }
}

impl<Dma: AsRef<DmaStream>> HasPaddr for DmaStreamSlice<Dma> {
    fn paddr(&self) -> Paddr {
        self.stream.as_ref().paddr() + self.offset
    }
}

impl Clone for DmaStreamSlice<DmaStream> {
    fn clone(&self) -> Self {
        Self {
            stream: self.stream.clone(),
            offset: self.offset,
            len: self.len,
        }
    }
}
