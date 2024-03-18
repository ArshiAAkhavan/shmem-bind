extern crate shmem_box as shmem;

use std::error::Error;
use std::mem;
use std::process::Command;

use shmem::ShmemBox;

#[derive(Debug)]
struct Data {
    val: i32,
}

impl Drop for Data {
    fn drop(&mut self) {
        println!("data is dropping");
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    // create new shared memory pointer with desired size
    //
    // first call to this function with the same FILE_LINK_ID would result in creating a new shared
    // memory file and owning it. this would result in deleting the shared memory when the variable
    // goes out of scope.
    // the second call to this function will only open shared memory and would not delete it.
    let shared_mem = shmem::Builder::new("shmem-example_message-passing.shm")
        .with_size(mem::size_of::<Data>() as i64)
        .open()?;

    // wrap the raw shared memory ptr with desired Boxed type
    // user must ensure that the data the pointer is pointing to is initialized and valid for use
    let mut data = unsafe { shared_mem.boxed::<Data>() };

    let mut args = std::env::args();
    let num_args = args.len();
    match num_args {
        // parent process
        1 => {
            // ensure that first process owns the shared memory (used for cleanup)
            let mut data = ShmemBox::own(data);

            // initiate the data behind the boxed pointer
            data.val = 1;

            let binary_path = args.next().unwrap();
            let new_val = 5;
            // create new process to mutate the shared memory
            let mut handle = Command::new(&binary_path)
                .arg(format!("{new_val}"))
                .spawn()
                .unwrap();
            handle.wait()?;

            // assert that the new process mutated the shared memory
            assert_eq!(data.val, new_val);
        }
        // child process
        2 => {
            let value = std::env::args().last().unwrap().parse()?;

            data.val = value;
            let _ = ShmemBox::leak(data);
        }
        _ => unimplemented!(),
    }
    Ok(())
}
