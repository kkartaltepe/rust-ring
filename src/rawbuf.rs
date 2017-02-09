pub struct RawBuf<T> {
	ptr: *mut T,
	size: usize,
}

impl<T> RawBuf<T> { //"Stable"
	pub fn new(size: usize) -> RawBuf<T> {
		let mut v = Vec::with_capacity(size);
		let ptr = v.as_mut_ptr();
		::std::mem::forget(v);

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
		unsafe{ ::std::mem::drop(Vec::from_raw_parts(self.ptr, 0, self.size)); }
	}

}