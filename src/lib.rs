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
    /// ensures a shared memory using the specified `size` and `flink_id` and mapping it to the
    /// virtual address of the process memory.
    ///
    /// in case of success, a `ShmemConf` is returned, representing the configuration of the
    /// allocated shared memory.
    ///
    /// if the shared memory with the given `flink_id` is not present on the system, the call to
    /// `open` would create a new shared memory and claims its ownership which is later used for
    /// cleanup of the shared memory.
    ///
    /// # Examples
    /// ```
    /// use std::mem;
    /// use shmem_bind::{self as shmem,ShmemError};
    ///
    /// fn main() -> Result<(),ShmemError>{
    ///     // shared_mem is the owner
    ///     let shared_mem = shmem::Builder::new("flink_test")
    ///         .with_size(mem::size_of::<i32>() as i64)
    ///         .open()?;
    ///     {
    ///         // shared_mem_barrow is not the owner
    ///         let shared_mem_barrow = shmem::Builder::new("flink_test")
    ///             .with_size(mem::size_of::<i32>() as i64)
    ///             .open()?;
    ///
    ///         // shared_mem_barrow goes out of scope, the shared memory is unmapped from virtual
    ///         // memory of the process.
    ///     }
    ///     // shared_mem goes out of scope, the shared memory is unmapped from virtual memory of
    ///     // the process. after that, the shared memory is unlinked from the system.
    ///     Ok(())
    /// }
    ///```
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

/// A representation of a ***mapped*** shared memory.
#[derive(Debug)]
pub struct ShmemConf {
    /// `flink_id` of the shared memory to be created on the system
    id: String,
    /// wether or not this `ShmemConf` is the owner of the shared memory.
    /// this field is set to true when the shared memory is created by this `ShmemConf`
    is_owner: bool,
    /// file descriptor of the allocated shared memory 
    fd: i32,
    /// pointer to the shared memory
    addr: NonNull<()>,
    /// size of the allocation
    size: i64,
}

impl ShmemConf {
    /// converts `ShmemConf`'s raw pointer to a boxed pointer of type `T`.
    ///
    /// # Safety
    ///
    /// this function is unsafe because there is no guarantee that the referred T is initialized.
    /// the caller must ensure that the value behind the pointer is initialized before use.
    ///
    /// # Examples
    /// ```
    /// use std::mem;
    /// use shmem_bind::{self as shmem,ShmemError};
    ///
    /// type NotZeroI32 = i32;
    ///
    /// fn main() -> Result<(),ShmemError>{
    ///     let shared_mem = shmem::Builder::new("flink_test_boxed")
    ///         .with_size(mem::size_of::<NotZeroI32>() as i64)
    ///         .open()?;
    ///
    ///     let boxed_val = unsafe {
    ///         // the allocated shared_memory is not initialized and thus, not guaranteed to be a
    ///         // valid `NotZeroI32`
    ///         let mut boxed_val = shared_mem.boxed::<NotZeroI32>();
    ///         // manually initialize the value in the unsafe block
    ///         *boxed_val = 5;
    ///         boxed_val
    ///     };
    ///
    ///     assert_eq!(*boxed_val, 5);
    ///
    ///     let shared_mem = shmem::Builder::new("flink_test_boxed")
    ///         .with_size(mem::size_of::<NotZeroI32>() as i64)
    ///         .open()?;
    ///
    ///     let mut boxed_barrow_val = unsafe { shared_mem.boxed::<NotZeroI32>() };
    ///
    ///     assert_eq!(*boxed_barrow_val, 5);
    ///
    ///     // changes to boxed_barrow_val would reflect to boxed_val as well since they both point
    ///     // to the same location.
    ///     *boxed_barrow_val = 3;
    ///     assert_eq!(*boxed_val, 3);
    ///     
    ///     Ok(())
    /// }
    ///
    /// ```
    pub unsafe fn boxed<T>(self) -> ShmemBox<T> {
        ShmemBox {
            ptr: self.addr.cast(),
            conf: self,
        }
    }
}

/// # Safety
///
/// shared memory is shared between processes.
/// if it can withstand multiple processes mutating it, it can sure handle a thread or two!
unsafe impl<T: Sync> Sync for ShmemBox<T> {}
unsafe impl<T: Send> Send for ShmemBox<T> {}

/// A safe and typed wrapper for shared memory
///
/// `ShmemBox<T>` wraps the underlying pointer to the shared memory and implements `Deref` and
/// `DerefMut` for T
///
/// when ShmemBox<T> goes out of scope, the cleanup process of the shared memory is done.
#[derive(Debug)]
pub struct ShmemBox<T> {
    ptr: NonNull<T>,
    conf: ShmemConf,
}

impl<T> ShmemBox<T> {
    /// owns the shared memory. this would result in shared memory cleanup when this pointer goes
    /// out of scope.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::mem;
    /// use shmem_bind::{self as shmem,ShmemError,ShmemBox};
    ///
    /// fn main() -> Result<(),ShmemError>{
    ///     // shared memory is created. `shared_mem` owns the shared memory
    ///     let shared_mem = shmem::Builder::new("flink_test_own")
    ///         .with_size(mem::size_of::<i32>() as i64)
    ///         .open()?;
    ///     let mut boxed_val = unsafe { shared_mem.boxed::<i32>() };
    ///     
    ///     // leaking the shared memory to prevent `shared_mem` from cleaning it up.
    ///     ShmemBox::leak(boxed_val);
    ///     
    ///     // shared memory is already present on the machine. `shared_mem` does not own the
    ///     // shared memory.
    ///     let shared_mem = shmem::Builder::new("flink_test_own")
    ///         .with_size(mem::size_of::<i32>() as i64)
    ///         .open()?;
    ///     let boxed_val = unsafe { shared_mem.boxed::<i32>() };
    ///
    ///     // own the shared memory to ensure it's cleanup when the shared_mem goes out of scope.
    ///     let boxed_val = ShmemBox::own(boxed_val);
    ///
    ///     // boxed_val goes out of scope, the shared memory is cleaned up
    ///     Ok(())
    /// }
    ///
    /// ```
    pub fn own(mut shmem_box: Self) -> Self {
        shmem_box.conf.is_owner = true;

        shmem_box
    }

    /// leaks the shared memory and prevents the cleanup if the ShmemBox is the owner of the shared
    /// memory.
    /// this function is useful when you want to create a shared memory which lasts longer than the
    /// process creating it.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::mem;
    /// use shmem_bind::{self as shmem,ShmemError,ShmemBox};
    ///
    /// fn main() -> Result<(),ShmemError>{
    ///     // shared memory is created. `shared_mem` owns the shared memory
    ///     let shared_mem = shmem::Builder::new("flink_test_leak")
    ///         .with_size(mem::size_of::<i32>() as i64)
    ///         .open()?;
    ///     let mut boxed_val = unsafe { shared_mem.boxed::<i32>() };
    ///     
    ///     // leaking the shared memory to prevent `shared_mem` from cleaning it up.
    ///     ShmemBox::leak(boxed_val);
    ///     
    ///     // shared memory is already present on the machine. `shared_mem` does not own the
    ///     // shared memory.
    ///     let shared_mem = shmem::Builder::new("flink_test_leak")
    ///         .with_size(mem::size_of::<i32>() as i64)
    ///         .open()?;
    ///     let boxed_val = unsafe { shared_mem.boxed::<i32>() };
    ///
    ///     // own the shared memory to ensure it's cleanup when the shared_mem goes out of scope.
    ///     let boxed_val = ShmemBox::own(boxed_val);
    ///
    ///     // boxed_val goes out of scope, the shared memory is cleaned up
    ///     Ok(())
    /// }
    ///
    /// ```
    pub fn leak(mut shmem_box: Self) {
        // disabling cleanup for shared memory
        shmem_box.conf.is_owner = false;
    }
}

impl<T> Drop for ShmemBox<T> {
    fn drop(&mut self) {
        if self.conf.is_owner {
            // # Safety
            //
            // if current process is the owner of the shared_memory,i.e. creator of the shared
            // memory, then it should clean up after, that is, it should drop the inner T
            unsafe { drop_in_place(self.ptr.as_mut()) };
        }
    }
}
impl Drop for ShmemConf {
    fn drop(&mut self) {
        // # Safety
        //
        // if current process is the owner of the shared_memory,i.e. creator of the shared
        // memory, then it should clean up after.
        // the procedure is as follow:
        // 1. unmap the shared memory from processes virtual address space.
        // 2. unlink the shared memory completely from the os if self is the owner
        // 3. close the file descriptor of the shared memory
        if unsafe { munmap(self.addr.as_ptr() as *mut c_void, self.size as usize) } != 0 {
            panic!("failed to unmap shared memory from the virtual memory space")
        }

        if self.is_owner {
            let storage_id: *const c_char = self.id.as_bytes().as_ptr() as *const c_char;
            if unsafe { shm_unlink(storage_id) } != 0 {
                panic!("failed to reclaim shared memory")
            }
        }

        if unsafe { close(self.fd) } != 0 {
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
    fn ownership() {
        #[derive(Debug)]
        struct Data {
            val: i32,
        }

        let shmconf = Builder::new("test-shmem-box-ownership")
            .with_size(std::mem::size_of::<Data>() as i64)
            .open()
            .unwrap();
        let mut data = unsafe { shmconf.boxed::<Data>() };
        assert_eq!(data.val, 0);
        data.val = 1;

        ShmemBox::leak(data);

        let shmconf = Builder::new("test-shmem-box-ownership")
            .with_size(std::mem::size_of::<Data>() as i64)
            .open()
            .unwrap();
        let data = unsafe { shmconf.boxed::<Data>() };
        assert_eq!(data.val, 1);

        let _owned_data = ShmemBox::own(data);
    }

    #[test]
    fn multi_thread() {
        struct Data {
            val: i32,
        }
        // create new shared memory pointer with desired size
        let shared_mem = Builder::new("test-shmem-box-multi-thread.shm")
            .with_size(std::mem::size_of::<Data>() as i64)
            .open()
            .unwrap();

        // wrap the raw shared memory ptr with desired Boxed type
        // user must ensure that the data the pointer is pointing to is initialized and valid for use
        let data = unsafe { shared_mem.boxed::<Data>() };

        // ensure that first process owns the shared memory (used for cleanup)
        let mut data = ShmemBox::own(data);

        // initiate the data behind the boxed pointer
        data.val = 1;

        let new_val = 5;
        std::thread::spawn(move || {
            // create new shared memory pointer with desired size
            let shared_mem = Builder::new("test-shmem-box-multi-thread.shm")
                .with_size(std::mem::size_of::<Data>() as i64)
                .open()
                .unwrap();

            // wrap the raw shared memory ptr with desired Boxed type
            // user must ensure that the data the pointer is pointing to is initialized and valid for use
            let mut data = unsafe { shared_mem.boxed::<Data>() };
            data.val = new_val;
        })
        .join()
        .unwrap();
        // assert that the new process mutated the shared memory
        assert_eq!(data.val, new_val);
    }
}
