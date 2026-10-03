//! A write-back cache of filesystem blocks over the [`Device`]: the least
//! recently used block is evicted (written first if dirty), and `flush`
//! writes every dirty block (see [`Fs`](crate::Fs) for when it does).

use alloc::collections::BTreeMap;
use alloc::vec;
use alloc::vec::Vec;

use crate::{Device, Error, Result};

/// Blocks kept (at 4 KiB blocks, 1 MiB).
const SLOTS: usize = 256;

struct Slot {
    block: u32,
    data: Vec<u8>,
    dirty: bool,
    used: u64,
}

pub struct Cache<D: Device> {
    pub dev: D,
    block_size: usize,
    slots: Vec<Slot>,
    /// Block number -> slot index.
    index: BTreeMap<u32, usize>,
    tick: u64,
}

impl<D: Device> Cache<D> {
    pub fn new(dev: D, block_size: usize) -> Self {
        Cache { dev, block_size, slots: Vec::new(), index: BTreeMap::new(), tick: 0 }
    }

    fn offset(&self, block: u32) -> u64 {
        block as u64 * self.block_size as u64
    }

    /// The slot holding `block`, read from the device unless `fresh` (the
    /// caller overwrites it whole).
    fn slot(&mut self, block: u32, fresh: bool) -> Result<usize> {
        self.tick += 1;
        if let Some(&i) = self.index.get(&block) {
            self.slots[i].used = self.tick;
            return Ok(i);
        }
        let i = if self.slots.len() < SLOTS {
            self.slots.push(Slot { block, data: vec![0; self.block_size], dirty: false, used: 0 });
            self.slots.len() - 1
        } else {
            let i = (0..self.slots.len()).min_by_key(|&i| self.slots[i].used).unwrap();
            self.write_back(i)?;
            self.index.remove(&self.slots[i].block);
            i
        };
        let off = self.offset(block);
        let slot = &mut self.slots[i];
        slot.block = block;
        slot.dirty = false;
        slot.used = self.tick;
        if fresh {
            slot.data.fill(0);
        } else if !self.dev.read(off, &mut slot.data) {
            // Leave the slot unused (never indexed) rather than wrong.
            slot.block = u32::MAX;
            slot.used = 0;
            return Err(Error::Io);
        }
        self.index.insert(block, i);
        Ok(i)
    }

    fn write_back(&mut self, i: usize) -> Result<()> {
        if self.slots[i].dirty {
            let off = self.offset(self.slots[i].block);
            if !self.dev.write(off, &self.slots[i].data) {
                return Err(Error::Io);
            }
            self.slots[i].dirty = false;
        }
        Ok(())
    }

    /// Look at `block`.
    pub fn read<T>(&mut self, block: u32, f: impl FnOnce(&[u8]) -> T) -> Result<T> {
        let i = self.slot(block, false)?;
        Ok(f(&self.slots[i].data))
    }

    /// Change `block` (written at the next flush).
    pub fn modify<T>(&mut self, block: u32, f: impl FnOnce(&mut [u8]) -> T) -> Result<T> {
        let i = self.slot(block, false)?;
        self.slots[i].dirty = true;
        Ok(f(&mut self.slots[i].data))
    }

    /// A newly allocated block: zeroed, without reading it.
    pub fn zero(&mut self, block: u32) -> Result<()> {
        let i = self.slot(block, true)?;
        self.slots[i].data.fill(0);
        self.slots[i].dirty = true;
        Ok(())
    }

    /// Write every dirty block, in block order.
    pub fn flush(&mut self) -> Result<()> {
        let dirty: Vec<usize> = self.index.values().copied().filter(|&i| self.slots[i].dirty).collect();
        let mut ok = true;
        for i in dirty {
            ok &= self.write_back(i).is_ok();
        }
        if ok { Ok(()) } else { Err(Error::Io) }
    }
}
