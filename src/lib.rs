use std::{
    error::Error,
    fmt::Display,
    mem::ManuallyDrop,
    ops::{Deref, DerefMut},
    ptr,
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

            // try open existing shared memory
            let fd = shm_open(storage_id, O_RDWR, S_IRUSR | S_IWUSR);

            // shared memory didn't exist
            if fd < 0 {
                let fd = shm_open(storage_id, O_RDWR | O_CREAT, S_IRUSR | S_IWUSR);

                if fd < 0 {
                    return Err(ShmemError::CreateFailedErr);
                }

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
            addr,
            size: self.size,
        })
    }
}

pub struct ShmemConf {
    id: String,
    is_owner: bool,
    fd: i32,
    addr: *mut c_void,
    size: i64,
}

impl ShmemConf {
    pub fn boxed<T>(self) -> ShmemBox<T> {
        ShmemBox {
            ptr: unsafe { ManuallyDrop::new(Box::from_raw(self.addr as *mut T)) },
            conf: self,
        }
    }
}

// SAFETY:
// shared memory is shared between processes.
// if it can withstand multiple processes mutating it, it can sure handle a thread or two!
unsafe impl<T> Sync for ShmemBox<T> where T: Sync {}
unsafe impl<T> Send for ShmemBox<T> where T: Send {}
pub struct ShmemBox<T> {
    ptr: ManuallyDrop<Box<T>>,
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

        shmem_box.conf.addr as *mut T
    }
}

impl<T> Drop for ShmemBox<T> {
    fn drop(&mut self) {
        unsafe {
            let _ = munmap(
                self.ptr.as_mut() as *mut T as *mut c_void,
                self.conf.size as usize,
            );
        }

        if self.conf.is_owner {
            let storage_id: *const c_char = self.conf.id.as_bytes().as_ptr() as *const c_char;
            // Safety:
            // if current process is the owner of the shared_memory,i.e. creator of the shared
            // memory, then it should clean up after.
            // the procedure is as follow:
            // 1. unmap the shared memory from processes virtual address space.
            // 2. unlink the shared memory completely from the os
            unsafe {
                let _ = shm_unlink(storage_id);
            }
            
            unsafe {
                ManuallyDrop::drop(&mut self.ptr);
            }
        }

        // we should close the file descriptor when dropping the pointer regardless of being its
        // owner or not
        unsafe {
            let _ = close(self.conf.fd);
        }
    }
}

impl<T> Deref for ShmemBox<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.ptr
    }
}

impl<T> DerefMut for ShmemBox<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.ptr
    }
}

#[derive(Debug)]
pub enum ShmemError {
    CreateFailedErr,
    AllocationFailedErr,
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
        let mut data = shmconf.boxed::<Data>();
        assert_eq!(data.val1, 0);
        data.val1 = 1;

        ShmemBox::leak(data);

        let shmconf = Builder::new("test-shmem")
            .with_size(std::mem::size_of::<Data>() as i64)
            .open()
            .unwrap();
        let data = shmconf.boxed::<Data>();
        assert_eq!(data.val1, 1);

        let _owned_data = ShmemBox::own(data);
    }
}
