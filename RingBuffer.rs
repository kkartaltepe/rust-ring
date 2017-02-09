use std::sync::{Arc, Mutex, Condvar};
use std::result::Result;
use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicUsize, Ordering};

use std::cmp::{min};
use std::thread;
use std::clone::Clone;

struct RingBuffer<T> {
	buf: UnsafeCell<Vec<T>>,
	size: usize, // size of the underlying buffer
	cap: usize,
	signal: Mutex<()>,
	wnotify: Condvar,
	rnotify: Condvar,

	head: AtomicUsize,
	rlock: Mutex<()>,
	tail: AtomicUsize,
	wlock: Mutex<()>,

}

unsafe impl<T> Sync for RingBuffer<T> {}
unsafe impl<T> Send for RingBuffer<T> {}

const BLOCK_SIZE: usize = 1;

impl<T> RingBuffer<T>
where T: Clone {
	pub fn new(capacity: usize) -> RingBuffer<T> {
		let size = capacity+1;
		
		let rb = RingBuffer{
			buf: UnsafeCell::new(Vec::with_capacity(size)),
			cap: capacity,
			size: size,
			signal: Mutex::new(()),
			wnotify: Condvar::new(),
			rnotify: Condvar::new(),

			head: AtomicUsize::new(0),
			wlock: Mutex::new(()),
			tail: AtomicUsize::new(0),
			rlock: Mutex::new(()),
		};
		unsafe {
			(*rb.buf.get()).set_len(capacity);
		} // Ensure raw buffer is allocated before this.
		return rb;
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
					drop(self.rnotify.wait(self.signal.lock().unwrap()).unwrap());
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
				drop(self.wnotify.wait(self.signal.lock().unwrap()).unwrap());
			} else {
				return free;
			}

		}
			
	}

	pub fn read_full(&self, buf: &mut Vec<T>) -> Result<(), String> {
		if buf.len() == 0 {
			return Ok(());
		}
		let rlock = self.rlock.lock().unwrap();

		let to_read = buf.len();
		let mut have_read = 0;
		
		let mut tail = self.tail.load(Ordering::Acquire);
		while have_read < to_read {
			let used = self.wait_used(min(BLOCK_SIZE,to_read));

			let self_buf = unsafe{ &*self.buf.get() };

			let readable = min(used, to_read-have_read);
			for i in 0..readable {
				buf.push(self_buf[(tail+i)%self.size].clone());
			}
			have_read += readable;

			tail = (tail + readable) % self_buf.len();
			self.tail.store(tail, Ordering::Release);
			self.wnotify.notify_one();
		}

		drop(rlock);
		return Ok(());
	}

	pub fn write_full(&self, buf: &[T]) -> Result<(), String> {
		if buf.len() == 0 {
			return Ok(());
		}
		let wlock = self.wlock.lock();

		let to_write = buf.len();
		let mut have_write = 0;

		let mut head = self.head.load(Ordering::Acquire);

		while have_write < to_write {
			let free = self.wait_free(min(BLOCK_SIZE, to_write));

			let self_buf = unsafe{ &mut *self.buf.get() };

			let writable = min(free, to_write-have_write);
			println!("Trying to write {}", writable);
			for i in 0..writable {
				 self_buf[(head+i)%self.size] = buf[have_write+i].clone();
			}
			println!("Successfully wrote {}", writable);

			have_write += writable;

			head = (head + writable) % self.size;
			self.head.store(head, Ordering::Release);
			self.rnotify.notify_one();
		}


		drop(wlock);
		return Ok(());
	}
}


fn main() {
	let ring = Arc::new(RingBuffer::<Arc<[u8; 1024*1024]>>::new(10));
	let mut threads = Vec::new();
	
	let start = std::time::Instant::now();
	for j in 0..1 {
		let r = ring.clone();
		let g = thread::spawn(move || {
			for i in 0..100usize {
				println!("About to prep data for {}:{}", j,i);
				let write_me = vec![Arc::new([j*2+(i%2) as u8; 1024*1024])];
				println!("writable size: {}", write_me.len());
				r.write_full(write_me.as_slice()).unwrap();

			}
		});
		threads.push(g);
	}

	std::thread::sleep(std::time::Duration::from_millis(4000));
	for _ in 0..10 {
		let r = ring.clone();
		let g = thread::spawn(move || {
			for _ in 0..100usize {
				let mut read_data = vec![];
				println!("Reading~");
				r.read_full(&mut read_data).unwrap();
				for v in read_data.iter() {
					for x in v.iter() {
						assert!(*x == (*read_data[0])[0]);
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
	let elems = 10*100*1024*1024;
	println!("{} elems took {}.{} nanos", elems, dur.as_secs(), dur.subsec_nanos());
	println!("{} elems/s", elems as f64 /(dur.as_secs() as f64 + 0.0000000001f64*(dur.subsec_nanos() as f64)));
}
