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

struct RawBuf<T> {
	ptr: *mut T,
	size: usize,
}

impl<T> RawBuf<T> { //"Stable"
	pub fn new(size: usize) -> RawBuf<T> {
		let mut v = Vec::with_capacity(size);
	    let ptr = v.as_mut_ptr();
	    std::mem::forget(v);

	    return RawBuf {
	    	ptr: ptr,
	    	size: size,
	    }
	}

	pub fn ptr(&self) -> *mut T {
		return self.ptr;
	}

	pub fn len(&self) -> usize {
		return self.size;
	}
}

impl<T> Drop for RawBuf<T> {
	// Drops the buffer *without* dropping any of the values possibly still stored.
	// User of this struct MUST be responsible for elements.
	fn drop(&mut self) {
		unsafe{ std::mem::drop(Vec::from_raw_parts(self.ptr, 0, self.size)); }
	}
}

struct RingBuffer<T> {
	buf: UnsafeCell<RawBuf<T>>,
	size: usize, // size of the underlying buffer
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
			size: size,

			head: AtomicUsize::new(0),
			wlock: Mutex::new(()),
			tail: AtomicUsize::new(0),
			rlock: Mutex::new(()),
		};
	}

	pub fn wait_used(&self, amt: usize) -> usize {
			loop  {
				let tail = self.tail.load(Ordering::Relaxed);
				let head = self.head.load(Ordering::Relaxed);
				let used = match head >= tail {
					true => (head - tail),
					false => ((head + self.size) - tail),
				};

				if used < amt {
					std::thread::yield_now();
					// drop(self.rnotify.wait(self.signal.lock().unwrap()).unwrap());
				} else {
					return used;
				}

			}

	}

	pub fn wait_free(&self, amt: usize) -> usize {
		loop  {
			let tail = self.tail.load(Ordering::Relaxed);
			let head = self.head.load(Ordering::Relaxed);
			let free = self.cap - match head >= tail {
				true => (head - tail),
				false => ((head + self.size) - tail),
			};

			if free < amt {
				std::thread::yield_now();
				// drop(self.wnotify.wait(self.signal.lock().unwrap()).unwrap());
			} else {
				return free;
			}

		}
			
	}

	#[inline]
	unsafe fn buf_read(&self, idx: usize) -> T {
		return ptr::read((*self.buf.get()).ptr().offset(idx as isize));
	}

	#[inline]
    unsafe fn buf_write(&self, idx: usize, value: T) {
        ptr::write((*self.buf.get()).ptr().offset(idx as isize), value);
    }

	pub fn read_full(&self, buf: &mut Vec<T>, amt: usize) -> Result<(), String> {
		let rlock = self.rlock.lock().unwrap();

		// let to_read = buf.len();
		let to_read = amt;
		let mut have_read = 0;

		let mut tail = self.tail.load(Ordering::Acquire);
		while have_read < to_read {
			let used = self.wait_used(min(BLOCK_SIZE,to_read-have_read));

			let readable = min(used, to_read-have_read);
			// println!("Reading {}/{}", readable+have_read, to_read);
			for i in 0..readable {
				unsafe{ buf.push(self.buf_read((tail+i)%self.size)); }
			}
			have_read += readable;

			tail = (tail + readable) % self.size;
			self.tail.store(tail, Ordering::Release);
			// self.wnotify.notify_one();
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
			// println!("Writing {}/{}", writable+have_write, to_write);
			for i in 0..writable {
				 unsafe{ self.buf_write((head+i)%self.size, buf[have_write+i].clone()); }
			}
			have_write += writable;

			head = (head + writable) % self.size;
			self.head.store(head, Ordering::Release);
			// self.rnotify.notify_one();
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
