use std::sync::{Arc, Mutex, Condvar, MutexGuard};
use std::result::Result;
use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicUsize, Ordering};

use std::cmp::{min};
use std::thread;

struct RingBuffer {
	buf: UnsafeCell<Vec<u8>>,
	cap: usize,
	used: Mutex<usize>,
	wnotify: Condvar,
	rnotify: Condvar,

	head: AtomicUsize,
	tail: AtomicUsize,
	wlock: Mutex<()>,
	rlock: Mutex<()>,
}

unsafe impl Sync for RingBuffer {}
unsafe impl Send for RingBuffer {}

const CACHE_BUF: usize = 128;

impl RingBuffer {
	pub fn new(capacity: usize) -> RingBuffer {
		return RingBuffer{
			buf: UnsafeCell::new(vec![0u8; capacity+CACHE_BUF]),
			cap: capacity,
			used: Mutex::new(0),
			wnotify: Condvar::new(),
			rnotify: Condvar::new(),

			head: AtomicUsize::new(0),
			tail: AtomicUsize::new(0),
			rlock: Mutex::new(()),
			wlock: Mutex::new(()),
		};
	}

	pub fn len(&self) -> usize {
			let tail = self.tail.load(Ordering::Relaxed);
			let head = self.head.load(Ordering::Relaxed);
			let self_buf = unsafe{ &*self.buf.get() };
			match head >= tail {
				true => (head - tail),
				false => ((head + self_buf.len()) - tail),
			}
	}

	pub fn read_full(&self, buf: &mut [u8]) -> Result<(), String> {
		if buf.len() == 0 {
			return Ok(());
		}
		let rlock = self.rlock.lock().unwrap();

		let to_read = buf.len();
		let mut have_read = 0;
		while have_read < to_read {
			let mut used = self.used.lock().unwrap();
			while *used < 1 {
				used = self.rnotify.wait(used).unwrap();
			}
			drop(used);

			let len = self.len();
			let tail = self.tail.load(Ordering::Acquire);
			let self_buf = unsafe{ &*self.buf.get() };
			let readable = min(len, to_read-have_read);
			for i in 0..readable {
				buf[have_read+i] = self_buf[(tail+i)%self_buf.len()];
			}
			self.tail.store((tail + readable) % self_buf.len(), Ordering::Release);

			let mut used = self.used.lock().unwrap();
			*used = self.len();
			self.wnotify.notify_one();
			drop(used);

			have_read += readable;

		}
		drop(rlock);
		return Ok(());
	}

	pub fn write_full(&self, buf: &[u8]) -> Result<(), String> {
		if buf.len() == 0 {
			return Ok(());
		}
		let wlock = self.wlock.lock();

		let to_write = buf.len();
		let mut have_write = 0;

		while have_write < to_write {
			let mut used = self.used.lock().unwrap();
			while self.cap - *used < 1 {
				used = self.wnotify.wait(used).unwrap();
			}
			drop(used);

			let head = self.head.load(Ordering::Acquire);
			let self_buf = unsafe{ &mut *self.buf.get() };
			let buf_len = self_buf.len();

			let free = self.cap - self.len();
			let writable = min(free, to_write-have_write);
			for i in 0..writable {
				 self_buf[(head+i)%buf_len] = buf[have_write+i];
			}
			have_write += writable;
			self.head.store((head + writable) % buf_len, Ordering::Release);

			let mut used = self.used.lock().unwrap();
			*used = self.len();
			self.rnotify.notify_one();
			drop(used);
		}
		drop(wlock);
		return Ok(());
	}
}


fn main() {
	let ring = Arc::new(RingBuffer::new(4*1024*1024));
	let mut threads = Vec::new();
	
	let start = std::time::Instant::now();
	for j in 0..10 {
		let r = ring.clone();
		let g = thread::spawn(move || {
			for i in 0..100usize {
				let write_me = [j*2+(i%2) as u8; 1024*1024];
				r.write_full(&write_me).unwrap();
			}
		});
		threads.push(g);
	}

	std::thread::sleep(std::time::Duration::from_millis(4000));
	for _ in 0..10 {
		let r = ring.clone();
		let g = thread::spawn(move || {
			for _ in 0..100usize {
				let mut read_data = [0; 1024*1024];
				r.read_full(&mut read_data).unwrap();
				for x in read_data.iter() {
					assert!(*x == read_data[0]);
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
	let elems = 10*100*1024*1024;
	println!("{} elems took {}.{} nanos", elems, dur.as_secs(), dur.subsec_nanos());
	println!("{} elems/s", elems as f64 /(dur.as_secs() as f64 + 0.0000000001f64*(dur.subsec_nanos() as f64)));
}
