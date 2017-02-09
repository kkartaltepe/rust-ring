use std::sync::{Arc, Mutex, Condvar, MutexGuard};
use std::result::Result;
use std::sync::atomic::{AtomicUsize, Ordering};

use std::cmp::{min};
use std::thread;

#[repr(C)]
struct RingBuffer {
	buf: Vec<Mutex<Option<u8>>>,
	cap: usize,
	buf5: i64,
	buf6: i64,
	head: AtomicUsize,
	wlock: Mutex<()>,
	buf1: i64,
	buf2: i64,
	tail: AtomicUsize,
	rlock: Mutex<()>,
	buf3: i64,
	buf4: i64,
	
	signal: Mutex<()>,
	wnotify: Condvar,
	rnotify: Condvar,
}

const CACHE_BUF: usize = 128;

unsafe impl Sync for RingBuffer {}
unsafe impl Send for RingBuffer {}

impl RingBuffer {
	pub fn new(capacity: usize) -> RingBuffer {
		let mut inner_buf = Vec::with_capacity(capacity+CACHE_BUF);
		let mut len = 0;
		while len <capacity {
			inner_buf.push(Mutex::new(None));
			len += 1;
		}

		return RingBuffer{
			buf: inner_buf,
			cap: capacity,
			head: AtomicUsize::new(0),
			tail: AtomicUsize::new(0),
			rlock: Mutex::new(()),
			wlock: Mutex::new(()),
			signal: Mutex::new(()),
			wnotify: Condvar::new(),
			rnotify: Condvar::new(),
			buf1: 0,
			buf2: 0,
			buf3: 0,
			buf4: 0,
			buf5: 0,
			buf6: 0,
		};
	}

	pub fn read_full(&self, buf: &mut [u8]) -> Result<(), String> {
		if buf.len() == 0 {
			return Ok(());
		}
		let rlock = self.rlock.lock().unwrap();

		let to_read = buf.len();
		let mut have_read = 0;
		let tail = self.tail.load(Ordering::Acquire);
		let ref self_buf = self.buf;
		let buf_len = self_buf.len();

		while have_read < to_read {
			let mut guard = self_buf[(tail+have_read)%buf_len].lock().unwrap();
			while guard.is_none() {
				drop(guard);
				drop(self.rnotify.wait(self.signal.lock().unwrap()).unwrap());
				guard = self_buf[(tail+have_read)%buf_len].lock().unwrap();
			}
			
			buf[have_read] = guard.unwrap();
			*guard = None;
			have_read += 1;

			self.wnotify.notify_one();
		}
		self.tail.store((tail+have_read)%self_buf.len(), Ordering::Release);

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
		let head = self.head.load(Ordering::Acquire);
		let ref self_buf = self.buf;
		let buf_len = self_buf.len();

		while have_write < to_write {
			let mut guard = self_buf[(head+have_write)%buf_len].lock().unwrap();
			while guard.is_some() {
				drop(guard);
				drop(self.wnotify.wait(self.signal.lock().unwrap()).unwrap());
				guard = self_buf[(head+have_write)%buf_len].lock().unwrap();
			}
		 	*guard = Some(buf[have_write]);
			have_write += 1;

			self.rnotify.notify_one();
		}
		self.head.store((head+have_write)%buf_len, Ordering::Release);

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
	println!("took {}.{} nanos", dur.as_secs(), dur.subsec_nanos())
}
