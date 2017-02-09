extern crate core;

use std::sync::{Arc, Mutex};
use std::result::Result;
use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicUsize, Ordering};

use std::cmp::{min};
use std::thread;
use std::clone::Clone;
use std::ops::Deref;

use core::ptr;

mod rawbuf;

use rawbuf::RawBuf;

struct RingBuffer<T> {
	buf: UnsafeCell<RawBuf<T>>,
	cap: usize,

	head: AtomicUsize,
	rlock: Mutex<()>,
	tail: AtomicUsize,
	wlock: Mutex<()>,

}

unsafe impl<T> Sync for RingBuffer<T> {}
unsafe impl<T> Send for RingBuffer<T> {}

// Probably best to leave this as is
const BLOCK_SIZE: usize = 1;

impl<T> RingBuffer<T>
where T: Clone {
	pub fn new(capacity: usize) -> RingBuffer<T> {
		let size = capacity+1;
		return RingBuffer{
			buf: UnsafeCell::new(RawBuf::new(size)),
			cap: capacity,

			head: AtomicUsize::new(0),
			wlock: Mutex::new(()),
			tail: AtomicUsize::new(0),
			rlock: Mutex::new(()),
		};

	}

	#[inline]
	unsafe fn buf_read(&self, idx: usize) -> T {
		return ptr::read((*self.buf.get()).ptr().offset(idx as isize));
	}

	#[inline]
	unsafe fn buf_write(&self, idx: usize, value: T) {
		ptr::write((*self.buf.get()).ptr().offset(idx as isize), value);
	}

	#[inline]
	fn buf_size(&self) -> usize {
		return unsafe{ &*self.buf.get() }.len();
	}

	#[inline]
	fn used(&self) -> usize {
		let tail = self.tail.load(Ordering::Relaxed);
		let head = self.head.load(Ordering::Relaxed);
		match head >= tail {
			true => (head - tail),
			false => ((head + self.buf_size()) - tail),
		}
	}

	#[inline]
	fn wait_used(&self, amt: usize) -> usize {
		loop {
			let used = self.used();

			if used < amt {
				std::thread::yield_now();
			} else {
				return used;
			}

		}

	}

	#[inline]
	fn wait_free(&self, amt: usize) -> usize {
		loop  {
			let free = self.cap - self.used();

			if free < amt {
				std::thread::yield_now();
			} else {
				return free;
			}

		}

	}

	fn read_full(&self, buf: &mut Vec<T>, amt: usize) -> Result<(), String> {
		let rlock = self.rlock.lock().unwrap();

		let to_read = amt;
		let mut have_read = 0;

		let mut tail = self.tail.load(Ordering::Acquire);
		while have_read < to_read {
			let used = self.wait_used(min(BLOCK_SIZE,to_read-have_read));

			let readable = min(used, to_read-have_read);
			for i in 0..readable {
				unsafe{ buf.push(self.buf_read((tail+i)%self.buf_size())); }
			}
			have_read += readable;

			tail = (tail + readable) % self.buf_size();
			self.tail.store(tail, Ordering::Release);
		}

		drop(rlock);
		return Ok(());
	}

	pub fn write_full(&self, buf: &[T]) -> Result<(), String> {
		if buf.len() == 0 {
			return Ok(());
		}
		let wlock = self.wlock.lock().unwrap();

		let to_write = buf.len();
		let mut have_write = 0;

		let mut head = self.head.load(Ordering::Acquire);

		while have_write < to_write {
			let free = self.wait_free(min(BLOCK_SIZE, to_write-have_write));

			let writable = min(free, to_write-have_write);
			for i in 0..writable {
				 unsafe{ self.buf_write((head+i)%self.buf_size(), buf[have_write+i].clone()); }
			}
			have_write += writable;

			head = (head + writable) % self.buf_size();
			self.head.store(head, Ordering::Release);
		}


		drop(wlock);
		return Ok(());
	}
}

const NUM_THREADS: usize = 10;
const NUM_WRITES: usize = 100;
const PIECES_PER_WRITE: usize = 512;
const PIECE_SIZE: usize = 1024;
const RING_SIZE: usize = 10;


fn main() {
	let ring = Arc::new(RingBuffer::<Arc<[u8; PIECE_SIZE]>>::new(RING_SIZE));
	let mut threads = Vec::new();
	
	let start = std::time::Instant::now();
	for j in 0..NUM_THREADS {
		let r = ring.clone();
		let g = thread::spawn(move || {
			for i in 0..NUM_WRITES {
				let mut write_me = Vec::with_capacity(PIECES_PER_WRITE);
				for k in 0..PIECES_PER_WRITE {
					write_me.push(Arc::new([(j*4+(i%2)*2+(k%2)) as u8; PIECE_SIZE]));
				}
				r.write_full(write_me.as_slice()).unwrap();
			}
		});
		threads.push(g);
	}

	for _ in 0..NUM_THREADS {
		let r = ring.clone();
		let g = thread::spawn(move || {
			for _ in 0..NUM_WRITES {
				let mut read_data = Vec::with_capacity(PIECES_PER_WRITE);
				r.read_full(&mut read_data, PIECES_PER_WRITE).unwrap();

				for v in 0..read_data.len() {
					if v > 0 {
						assert!(read_data[v].deref()[0]%2 != read_data[v-1].deref()[0]%2)
					}
					for x in read_data[v].iter() {
						assert!(*x == read_data[v].deref()[0]);
					}
				}
			}
		});
		threads.push(g);
	}

	for t in threads {
		t.join().unwrap();
	}

	let end = std::time::Instant::now();
	let dur = end.duration_since(start);
	let elems = NUM_THREADS*NUM_WRITES*PIECES_PER_WRITE*PIECE_SIZE;
	println!("{} elems took {}.{} nanos", elems, dur.as_secs(), dur.subsec_nanos());
	println!("{} elems/s", elems as f64 /(dur.as_secs() as f64 + 0.0000000001f64*(dur.subsec_nanos() as f64)));
}