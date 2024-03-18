use std::{
    error::Error,
    fmt::Display,
    ops::{Deref, DerefMut},
    ptr::{self, drop_in_place, NonNull},
};

use libc::{
    c_char, c_void, close, ftruncate, mmap, munmap, shm_open, shm_unlink, MAP_SHARED, O_CREAT,
    O_RDWR, PROT_WRITE, S_IRUSR, S_IWUSR,
};

pub struct Builder {
    id: String,
}

impl Builder {
    pub fn new(id: &str) -> Self {
        Self {
            id: String::from(id),
        }
    }

    pub fn with_size(self, size: i64) -> BuilderWithSize {
        BuilderWithSize { id: self.id, size }
    }
}

pub struct BuilderWithSize {
    id: String,
    size: i64,
}
impl BuilderWithSize {
    pub fn open(self) -> Result<ShmemConf, ShmemError> {
        let (fd, is_owner) = unsafe {
            let storage_id: *const c_char = self.id.as_bytes().as_ptr() as *const c_char;

            // open the existing shared memory if exists
            let fd = shm_open(storage_id, O_RDWR, S_IRUSR | S_IWUSR);

            // shared memory didn't exist
            if fd < 0 {
                // create the shared memory
                let fd = shm_open(storage_id, O_RDWR | O_CREAT, S_IRUSR | S_IWUSR);
                if fd < 0 {
                    return Err(ShmemError::CreateFailedErr);
                }

                // allocate the shared memory with required size
                let res = ftruncate(fd, self.size);
                if res < 0 {
                    return Err(ShmemError::AllocationFailedErr);
                }

                (fd, true)
            } else {
                (fd, false)
            }
        };

        let null = ptr::null_mut();
        let addr = unsafe { mmap(null, self.size as usize, PROT_WRITE, MAP_SHARED, fd, 0) };

        Ok(ShmemConf {
            id: self.id,
            is_owner,
            fd,
            addr: NonNull::new(addr as *mut _).ok_or(ShmemError::NullPointerErr)?,
            size: self.size,
        })
    }
}

#[derive(Debug)]
pub struct ShmemConf {
    id: String,
    is_owner: bool,
    fd: i32,
    addr: NonNull<()>,
    size: i64,
}

impl ShmemConf {
    // SAFETY: this is unsafe because there is no guarantee that the referred T is initialized or
    // valid
    pub unsafe fn boxed<T>(self) -> ShmemBox<T> {
        ShmemBox {
            ptr: self.addr.cast(),
            conf: self,
        }
    }
}

// SAFETY:
// shared memory is shared between processes.
// if it can withstand multiple processes mutating it, it can sure handle a thread or two!
unsafe impl<T: Sync> Sync for ShmemBox<T> {}
unsafe impl<T: Send> Send for ShmemBox<T> {}

#[derive(Debug)]
pub struct ShmemBox<T> {
    ptr: NonNull<T>,
    conf: ShmemConf,
}

impl<T> ShmemBox<T> {
    // owns the shared memory. this would result in shared memory cleanup when this pointer goes
    // out of scope
    pub fn own(mut shmem_box: Self) -> Self {
        shmem_box.conf.is_owner = true;

        shmem_box
    }

    // leaks the shared memory and prevents the cleanup if the ShmemBox is the owner of the shared
    // memory.
    // this function is useful when you want to create a shared memory which last longer than the
    // process creating it.
    pub fn leak(mut shmem_box: Self) -> *mut T {
        // disabling cleanup for shared memory
        shmem_box.conf.is_owner = false;

        let addr = shmem_box.ptr.as_ptr();
        std::mem::forget(shmem_box);
        addr
    }
}

impl<T> Drop for ShmemBox<T> {
    fn drop(&mut self) {
        if self.conf.is_owner {
            let storage_id: *const c_char = self.conf.id.as_bytes().as_ptr() as *const c_char;
            // Safety:
            // if current process is the owner of the shared_memory,i.e. creator of the shared
            // memory, then it should clean up after.
            // the procedure is as follow:
            // 1. drop the inner T
            // 2. unmap the shared memory from processes virtual address space.
            // 3. unlink the shared memory completely from the os
            unsafe { drop_in_place(self.ptr.as_mut()) };
            if unsafe { munmap(self.ptr.as_ptr() as *mut c_void, self.conf.size as usize) } != 0 {
                panic!("failed to unmap shared memory from the virtual memory space")
            }
            if unsafe { shm_unlink(storage_id) } != 0 {
                panic!("failed to reclaim shared memory")
            }
        } else {
            if unsafe { munmap(self.ptr.as_ptr() as *mut c_void, self.conf.size as usize) } != 0 {
                panic!("failed to unmap shared memory from the virtual memory space")
            }
        }

        // we should close the file descriptor when dropping the pointer regardless of being its
        // owner or not
        if unsafe { close(self.conf.fd) } != 0 {
            panic!("failed to close shared memory file descriptor")
        }
    }
}

impl<T> Deref for ShmemBox<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { self.ptr.as_ref() }
    }
}

impl<T> DerefMut for ShmemBox<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { self.ptr.as_mut() }
    }
}

#[derive(Debug)]
pub enum ShmemError {
    CreateFailedErr,
    AllocationFailedErr,
    NullPointerErr,
}
impl Display for ShmemError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl Error for ShmemError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_works() {
        struct Data {
            val1: i32,
            _val2: f32,
            _val3: [u8; 12],
        }
        let shmconf = Builder::new("test-shmem")
            .with_size(std::mem::size_of::<Data>() as i64)
            .open()
            .unwrap();
        let mut data = unsafe { shmconf.boxed::<Data>() };
        assert_eq!(data.val1, 0);
        data.val1 = 1;

        ShmemBox::leak(data);

        let shmconf = Builder::new("test-shmem")
            .with_size(std::mem::size_of::<Data>() as i64)
            .open()
            .unwrap();
        let data = unsafe { shmconf.boxed::<Data>() };
        assert_eq!(data.val1, 1);

        let _owned_data = ShmemBox::own(data);
    }
}
