use std::error::Error;
use std::mem::{self, ManuallyDrop};
use std::process::Command;

use shmem::ShmemBox;

extern crate shmem_box as shmem;

#[derive(Debug)]
struct Data {
    val1: i32,
}

impl Drop for Data{
    fn drop(&mut self) {
        println!("data is dropping");
    }
}

fn main() -> Result<(), Box<dyn Error>> {
unsafe{
    let mut x = ManuallyDrop::new(Data{val1 : 3});
    ManuallyDrop::drop(&mut x);
    dbg!(x);


    }

    // create new shared memory pointer with desired size
    let shared_mem = shmem::Builder::new("shmem-example_message-passing.shm")
        .with_size(mem::size_of::<Data>() as i64)
        .open()?;
    
    // wrap the raw shared memory ptr with safe typed ShmemBox
    let mut data = unsafe { shared_mem.boxed::<Data>() };

    let mut args = std::env::args();
    let num_args = args.len();
    match num_args {
        // parent process
        1 => {
            // ensure that first process owns the shared memory (used for cleanup)
            let mut data = ShmemBox::own(data);
            
            // initiate the data behind the boxed pointer
            data.val1 = 1;

            let binary_path = args.next().unwrap();
            let new_val = 5;
            // create new process to mutate the shared memory
            let mut handle = Command::new(&binary_path)
                .arg(format!("{new_val}"))
                .spawn()
                .unwrap();
            handle.wait()?;

            // assert that the new process mutated the shared memory
            assert_eq!(data.val1, new_val);
        }
        // child process
        2 => {
            let value = std::env::args().last().unwrap().parse()?;

            data.val1 = value;
            let _ = ShmemBox::leak(data);
        }
        _ => unimplemented!(),
    }
    Ok(())
}
