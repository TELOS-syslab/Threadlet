#include <linux/module.h>
#include <linux/kernel.h>
#include <linux/mm.h>
#include <linux/irq.h>
#include <linux/sched.h>
#include <linux/cdev.h>
#include <linux/fs.h>

#define DEV_MAJOR            451
#define DEV_MINOR            0
#define DEV_NAME             "thlet_intr"
#define thlet_intr_ITER      10
#define thlet_intr_THREADS   1024
#define THLET_RUNNING_CORE   19
#define THLET_STATS_CORE     ((THLET_RUNNING_CORE) > 16 ? THLET_RUNNING_CORE - 16 : 16 - THLET_RUNNING_CORE)
#define THLET_SLEEP_NS       10000
#define THLET_LOOPS          1000

#define THLET_IOCTL_START _IO(DEV_MAJOR, 0)
#define THLET_IOCTL_FINISH _IO(DEV_MAJOR, 1)
#define THLET_IOCTL_CLEAR _IO(DEV_MAJOR, 2)
#define THLET_IOCTL_PRINT _IO(DEV_MAJOR, 3)

static int thlet_intr_helper_mmap(struct file *filp, struct vm_area_struct *vma);
static int thlet_intr_helper_open(struct inode *threadlet_inode, struct file *threadlet_file);
static int thlet_intr_helper_release(struct inode *threadlet_inode, struct file *threadlet_file);
static ssize_t thlet_intr_helper_read(struct file *p_file, char *u_buffer, size_t count, loff_t *ppos);
static ssize_t thlet_intr_helper_write(struct file *p_file, const char *u_buffer, size_t count, loff_t *ppos);
static long thlet_intr_helper_ioctl(struct file *file, unsigned ioctl_num, unsigned long ioctl_param);
static long thlet_intr_helper_ioctl32(struct file *file, unsigned ioctl_num, unsigned long ioctl_param);

struct file_operations thlet_intr_helper_fops = {
  .owner = THIS_MODULE,
  .read = thlet_intr_helper_read,
  .unlocked_ioctl = thlet_intr_helper_ioctl,
  .compat_ioctl = thlet_intr_helper_ioctl32,
  .write = thlet_intr_helper_write,
  .open = thlet_intr_helper_open,
  .release = thlet_intr_helper_release,
  .mmap = thlet_intr_helper_mmap,
};

thlet_syscall_record *stats = NULL;

static int thlet_intr_helper_mmap(struct file *filp, struct vm_area_struct *vma) {
    // 1. 如果还没分配内存，分配一个完整的物理页
    if (!stats) {
        stats = (void *)get_zeroed_page(GFP_KERNEL);
        if (!stats) return -ENOMEM;
        
        // 只有 alloc_page 分配的页才能安全地 SetPageReserved
        SetPageReserved(virt_to_page(stats));
    }

    // 2. 获取物理页帧号 (PFN)
    unsigned long pfn = virt_to_phys(stats) >> PAGE_SHIFT;
    unsigned long size = vma->vm_end - vma->vm_start;

    // 3. 检查用户请求的大小（不能超过一页）
    if (size > PAGE_SIZE)
        return -EINVAL;

    // 4. 映射到用户空间
    if (remap_pfn_range(vma, vma->vm_start, pfn, size, vma->vm_page_prot))
        return -EAGAIN;

    return 0;
}

static int thlet_intr_helper_open(struct inode *p_inode, struct file *p_file) {
  return 0;
}

static int thlet_intr_helper_release(struct inode *p_inode, struct file *p_file) {
  return 0;
}

static long thlet_intr_helper_ioctl32(struct file *file, unsigned ioctl_num, unsigned long ioctl_param) {
  unsigned long param = (unsigned long)((void *)(ioctl_param));

  thlet_intr_helper_ioctl(file, ioctl_num, param);
  return 0;
}

static ssize_t thlet_intr_helper_read(struct file *p_file, char *u_buffer, size_t count, loff_t *ppos) {
  return 0;
}

static ssize_t thlet_intr_helper_write(struct file *p_file, const char *u_buffer, size_t count, loff_t *ppos) {
  return 0;
}

static long thlet_intr_helper_ioctl(struct file *file, unsigned int ioctl_num, unsigned long ioctl_param) {
  uint64_t cpu;
  thlet_syscall_record *lstats;

  for_each_possible_cpu(cpu) {
    // printk("=================\n");
    lstats = &per_cpu(thlet_syscall_bottom_half, cpu);
    // printk("exception_entry %llu\n", lstats->exception_entry);
    // printk("do_trap_entry %llu\n", lstats->do_trap_entry);
    // printk("enter_from_user_entry %llu\n", lstats->enter_from_user_entry);
    // printk("enter_from_user_exit %llu\n", lstats->enter_from_user_exit);
    // printk("do_intr_entry %llu\n", lstats->do_intr_entry);
    // printk("do_intr_exit %llu\n", lstats->do_intr_exit);
    // printk("exit_to_user_entry %llu\n", lstats->exit_to_user_entry);
    // printk("exit_to_user_exit %llu\n", lstats->exit_to_user_exit);

    if (lstats->exception_entry != 0) {
      (* stats) = (* lstats);
      lstats->exception_entry = 0;
    }
  }
  return 0;
}

dev_t thlet_intr_helper_dev;
struct cdev *thlet_intr_helper_cdev;
struct class *thlet_class;

static int thlet_intr_helper_init(void) {
  printk("thlet_intr_helper initialized");

  thlet_intr_helper_dev = MKDEV(DEV_MAJOR, DEV_MINOR);
  register_chrdev_region(thlet_intr_helper_dev, 1, DEV_NAME);

  thlet_intr_helper_cdev = cdev_alloc();
  thlet_intr_helper_cdev->owner = THIS_MODULE;
  thlet_intr_helper_cdev->ops = &thlet_intr_helper_fops;
  cdev_init(thlet_intr_helper_cdev, &thlet_intr_helper_fops);
  cdev_add(thlet_intr_helper_cdev, thlet_intr_helper_dev, 1);

  thlet_class = class_create("thlet_class");
  device_create(thlet_class, NULL, thlet_intr_helper_dev, NULL, DEV_NAME);

  return 0;
}

static void thlet_intr_helper_exit(void) {
  if (stats) {
    ClearPageReserved(virt_to_page(stats));
    free_page((unsigned long)stats);
}

  cdev_del(thlet_intr_helper_cdev);
  unregister_chrdev_region(thlet_intr_helper_dev, 1);

  device_destroy(thlet_class, thlet_intr_helper_dev);
  class_destroy(thlet_class);
  printk("thlet_intr_helper removed");
}

module_init(thlet_intr_helper_init);
module_exit(thlet_intr_helper_exit);
MODULE_LICENSE("GPL");