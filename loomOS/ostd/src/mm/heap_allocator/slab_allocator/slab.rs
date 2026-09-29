// SPDX-License-Identifier: MPL-2.0

// Modified from slab.rs in slab_allocator project
//
// MIT License
//
// Copyright (c) 2024 Asterinas Developers
// Copyright (c) 2024 ArceOS Developers
// Copyright (c) 2017 Robert Węcławski
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is

//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.
//

use alloc::alloc::{AllocError, Layout};

use super::SET_SIZE;

pub struct Slab<const BLK_SIZE: usize> {
    free_block_list: FreeBlockList<BLK_SIZE>,
    total_blocks: usize,
}

impl<const BLK_SIZE: usize> Slab<BLK_SIZE> {
    pub unsafe fn new(start_addr: usize, slab_size: usize) -> Slab<BLK_SIZE> {
        let num_of_blocks = slab_size / BLK_SIZE;
        Slab {
            free_block_list: FreeBlockList::new(start_addr, BLK_SIZE, num_of_blocks),
            total_blocks: num_of_blocks,
        }
    }

    #[allow(unused)]
    pub fn total_blocks(&self) -> usize {
        self.total_blocks
    }

    #[allow(unused)]
    pub fn used_blocks(&self) -> usize {
        self.total_blocks - self.free_block_list.len()
    }

    pub unsafe fn grow(&mut self, start_addr: usize, slab_size: usize) {
        let num_of_blocks = slab_size / BLK_SIZE;
        self.total_blocks += num_of_blocks;
        let mut block_list = FreeBlockList::<BLK_SIZE>::new(start_addr, BLK_SIZE, num_of_blocks);
        while let Some(block) = block_list.pop() {
            self.free_block_list.push(block);
        }
    }

    pub fn allocate(
        &mut self,
        _layout: Layout,
        buddy: &mut buddy_system_allocator::Heap<32>,
    ) -> Result<usize, AllocError> {
        match self.free_block_list.pop() {
            Some(block) => {
                let addr = block as usize;
                Ok(addr)
            }
            None => {
                let layout = unsafe {
                    Layout::from_size_align_unchecked(SET_SIZE * BLK_SIZE, 4096)
                };
                // debug prints for buddy alloc request disabled
                if let Ok(ptr) = buddy.alloc(layout) {
                    let p = ptr.as_ptr() as usize;
                    // debug prints for buddy alloc ok disabled
                    unsafe {
                        self.grow(p, SET_SIZE * BLK_SIZE);
                    }
                    let _head_after = self.free_block_list.head_addr();
                    let blk = self.free_block_list.pop().unwrap();
                    let addr = blk as usize;
                    Ok(addr)
                } else {
                    Err(AllocError)
                }
            }
        }
    }

    pub fn deallocate(&mut self, ptr: usize) {
        let ptr = ptr as *mut FreeBlock;
        unsafe {
            // Reinitialize header before pushing back to free list.
            core::ptr::write(ptr, FreeBlock { next: core::ptr::null_mut() });
            self.free_block_list.push(ptr);
        }
    }
}

struct FreeBlockList<const BLK_SIZE: usize> {
    len: usize,
    head: *mut FreeBlock,
}

impl<const BLK_SIZE: usize> FreeBlockList<BLK_SIZE> {
    unsafe fn new(
        start_addr: usize,
        block_size: usize,
        num_of_blocks: usize,
    ) -> FreeBlockList<BLK_SIZE> {
        // debug prints for free block list creation disabled
        let mut new_list = FreeBlockList::new_empty();
        for i in (0..num_of_blocks).rev() {
            let new_block = (start_addr + i * block_size) as *mut FreeBlock;
            // Initialize the memory before creating a mutable reference.
            core::ptr::write(new_block, FreeBlock { next: core::ptr::null_mut() });
            // selective debug prints disabled
            new_list.push(new_block);
        }
        new_list
    }

    fn new_empty() -> FreeBlockList<BLK_SIZE> {
        FreeBlockList { len: 0, head: core::ptr::null_mut() }
    }

    fn len(&self) -> usize {
        self.len
    }

    fn pop(&mut self) -> Option<*mut FreeBlock> {
        if self.head.is_null() {
            return None;
        }
        let node = self.head;
        let addr = node as usize;
        unsafe {
            self.head = (*node).next;
        }
        self.len -= 1;
        Some(node)
    }

    fn push(&mut self, free_block: *mut FreeBlock) {
        let blk_addr = free_block as usize;
        let _head_before = self.head_addr();
        let _len_before = self.len;
        unsafe {
            (*free_block).next = self.head;
        }
        self.len += 1;
        self.head = free_block;
        // debug prints for push done disabled
    }

    fn head_addr(&self) -> usize { self.head as usize }

    #[allow(dead_code)]
    fn is_empty(&self) -> bool {
        self.head.is_null()
    }
}

struct FreeBlock { next: *mut FreeBlock }

impl FreeBlock {}

// SAFETY: Slab and its internal free list are only accessed behind SpinLock in the allocator,
// and we never share interior references across threads. Raw pointers are used as opaque indices.
unsafe impl<const BLK_SIZE: usize> Send for FreeBlockList<BLK_SIZE> {}
unsafe impl<const BLK_SIZE: usize> Send for Slab<BLK_SIZE> {}
