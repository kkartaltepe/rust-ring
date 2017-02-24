use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::cell::UnsafeCell;
use std::result::Result;
use std::thread;
use std::thread::Thread;
// use std::time::{Instant, Duration};

use std::cmp::{min};
use std::clone::Clone;

use core::ptr;
use rawbuf::RawBuf;

pub struct RingBuffer<T> {
	buf: UnsafeCell<RawBuf<T>>,
	cap: usize,


	waiting: Mutex<Option<(bool, Thread)>>,

	head: AtomicUsize,
	rlock: Mutex<()>,
	tail: AtomicUsize,
	wlock: Mutex<()>,

}

unsafe impl<T> Sync for RingBuffer<T> {}
unsafe impl<T> Send for RingBuffer<T> {}

// Probably best to leave this as is
const BLOCK_SIZE: usize = 5;

impl<T> RingBuffer<T>
where T: Clone {
	pub fn new(capacity: usize) -> RingBuffer<T> {
		let size = capacity+1;
		return RingBuffer{
			buf: UnsafeCell::new(RawBuf::new(size)),
			cap: capacity,

			waiting: Mutex::new(None),

			head: AtomicUsize::new(0),
			wlock: Mutex::new(()),
			tail: AtomicUsize::new(0),
			rlock: Mutex::new(()),
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

	fn wait(&self) {
		// Wait for other thread to finish starting.
		let mut wait_lock = self.waiting.lock().unwrap();
		// let start = Instant::now();
		loop {
			if wait_lock.is_none() { break; }

			let (waiting,wait_thread) = wait_lock.as_ref().unwrap().clone();
			if waiting {
				// println!("{:?} Trying to wake thread {:?}", thread::current(), wait_thread);
				*wait_lock = Some((false, wait_thread.clone()));
			}

			drop(wait_lock);
			wait_thread.unpark();
			thread::yield_now();

			// if Instant::now().duration_since(start) > Duration::from_millis(5000) {
			// 	panic!("Trying thread {:?} stuck waiting for thread {:?}", thread::current(), wait_thread);
			// }

			wait_lock = self.waiting.lock().unwrap();
		}

		// println!("Parking thread {:?}", thread::current());
		*wait_lock = Some((true, thread::current()));
		drop(wait_lock);

		loop { // Park until we are signaled to startup
			thread::park();

			wait_lock = self.waiting.lock().unwrap();
			if wait_lock.is_some() {
				if wait_lock.as_ref().unwrap().0 == false {
					// println!("{:?} Clearing waiter", thread::current());
					*wait_lock = None;
					break;
				}
			}
			drop(wait_lock);
		}
	}

	#[inline]
	fn wait_used(&self, amt: usize) -> usize {
		let used = self.used();

		if used < amt {
			self.wait();
			return self.used();
		} else {
			return used;
		}

	}

	#[inline]
	fn wait_free(&self, amt: usize) -> usize {
		let free = self.cap - self.used();

		if free < amt {
			self.wait();
			return self.cap - self.used();
		} else {
			return free;
		}

	}

	#[inline]
	fn unpark_thread(&self) {
		let mut wait_lock = self.waiting.lock().unwrap();
		if wait_lock.is_none() { return; }

		let (_, wait_thread) = wait_lock.as_ref().unwrap().clone();
		*wait_lock = Some((false, wait_thread.clone()));
		drop(wait_lock);
		wait_thread.unpark();
	}

	pub fn read_full(&self, amt: usize) -> Result<Vec<T>, String> {
		let rlock = self.rlock.lock().unwrap();

		let to_read = amt;
		let mut have_read = 0;
		let mut ret = Vec::with_capacity(amt);

		let mut tail = self.tail.load(Ordering::Acquire);
		while have_read < to_read {
			let used = self.wait_used(min(BLOCK_SIZE,to_read-have_read));

			let readable = min(used, to_read-have_read);
			for i in 0..readable {
				unsafe{ ret.push(self.buf_read((tail+i)%self.buf_size())); }
			}
			have_read += readable;

			tail = (tail + readable) % self.buf_size();
			self.tail.store(tail, Ordering::Release);
			self.unpark_thread();
		}

		drop(rlock);
		return Ok(ret);
	}

	pub fn read(&self, amt: usize) -> Result<Vec<T>, String> {
		let mut ret = Vec::new();
		let rlock = match self.rlock.try_lock() {
			Err(_) => return Ok(ret),
			g => g,
		};

		let to_read = amt;

		let mut tail = self.tail.load(Ordering::Acquire);
		let used = self.used();
		if used == 0 {
			return Ok(ret);
		}
		// let used = self.wait_used(min(BLOCK_SIZE,to_read-have_read));

		let readable = min(used, to_read);
		for i in 0..readable {
			unsafe{ ret.push(self.buf_read((tail+i)%self.buf_size())); }
		}

		tail = (tail + readable) % self.buf_size();
		self.tail.store(tail, Ordering::Release);
		self.unpark_thread();

		drop(rlock);
		return Ok(ret);
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
			self.unpark_thread();
		}

		drop(wlock);
		return Ok(());
	}

}

#[cfg(test)]
mod tests {
	use std::sync::Arc;
	use std::ops::Deref;
	use std::thread;
	use std::time::Instant;

	use ringbuffer::RingBuffer;

	const NUM_THREADS: usize = 10;
	const NUM_WRITES: usize = 100;
	const PIECES_PER_WRITE: usize = 1;
	const PIECE_SIZE: usize = 1024;
	const RING_SIZE: usize = 10;

	#[test]
	fn mixed_read_writes_no_interleaving() {
		let ring = Arc::new(RingBuffer::<Box<[u8]>>::new(RING_SIZE));
		let mut threads = Vec::new();
		let start = Instant::now();

		for j in 0..NUM_THREADS {
			let r = ring.clone();

			let builder = thread::Builder::new();
			let g = builder.name(format!("writer {}", j)).spawn(move || {
				for i in 0..NUM_WRITES {
					let mut write_me = Vec::<Box<[u8]>>::with_capacity(PIECES_PER_WRITE);
					for k in 0..PIECES_PER_WRITE {
						write_me.push(Box::new([(j*4+(i%2)*2+(k%2)) as u8; PIECE_SIZE]));
					}
					r.write_full(write_me.as_slice()).unwrap();
				}
			}).unwrap();
			threads.push(g);
		}

		for j in 0..NUM_THREADS {
			let r = ring.clone();

			let builder = thread::Builder::new();
			let g = builder.name(format!("reader {}", j)).spawn(move || {
				for _ in 0..NUM_WRITES {
					let read_data =	r.read_full(PIECES_PER_WRITE).unwrap();

					for v in 0..read_data.len() {
						if v > 0 {
							assert!(read_data[v].deref()[0]%2 != read_data[v-1].deref()[0]%2)
						}
						for x in read_data[v].iter() {
							assert!(*x == read_data[v].deref()[0]);
						}
					}
				}
			}).unwrap();
			threads.push(g);
		}

		// No join_try so pray this doesnt deadlock (since it shouldnt).
		// Maybe add timer in another thread and panic or something?
		for t in threads {
			assert!(t.join().is_ok());
		}
		let runtime = start.elapsed();
		println!("Ran MPMC in {}.{:09}", runtime.as_secs(), runtime.subsec_nanos());
	}

	#[test]
	fn single_read_write() {
		let ring = Arc::new(RingBuffer::<Box<[u8]>>::new(RING_SIZE));
		let mut threads = Vec::new();
		let start = Instant::now();
		
		for j in 0..1 {
			let r = ring.clone();
			let g = thread::spawn(move || {
				for i in 0..NUM_WRITES*NUM_THREADS { // Even out with data through multi threaded case
					let mut write_me = Vec::<Box<[u8]>>::with_capacity(PIECES_PER_WRITE);
					for k in 0..PIECES_PER_WRITE {
						write_me.push(Box::new([(j*4+(i%2)*2+(k%2)) as u8; PIECE_SIZE]));
					}
					r.write_full(write_me.as_slice()).unwrap();
				}
			});
			threads.push(g);
		}

		for _ in 0..1 {
			let r = ring.clone();
			let g = thread::spawn(move || {
				for _ in 0..NUM_WRITES*NUM_THREADS {
					let read_data =	r.read_full(PIECES_PER_WRITE).unwrap();

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

		// No join_try so pray this doesnt deadlock (since it shouldnt).
		// Maybe add timer in another thread and panic or something?
		for t in threads {
			assert!(t.join().is_ok());
		}
		let runtime = start.elapsed();
		println!("Ran SPSC in {}.{:09}", runtime.as_secs(), runtime.subsec_nanos());
	}
}
