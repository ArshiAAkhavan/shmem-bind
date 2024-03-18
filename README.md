# ShmemBox

A safe and ideomatic abstraction over shared memory APIs in rust

## Quick start

check the `message-passing` example for better understanding
```bash
cargo run --example message-passing
```

## Semantics:

in order to create new shared memory, use the following builder snippet:
```rust 
let shared_mem = shmem::Builder::new("<FLINK_FILE_HANDLE>")
    .with_size(mem::size_of::<Message>() as i64)
    .open()?;
```
this will allocate a shared memory file with the specified size if the shared memory is not present on the machine.
the handle, here `shared_mem`, would claim ownership of the shared memory if the shared memory is not present and created via call to `open` function.
this is useful information for cleanup process since there is only one owner for each shared memory and only the owner can and will unlink the shared memory.

you can wrap the shared memory configuration into a `ShmemBox<T>` via call to `boxed` function.
```rust
let boxed_val = unsafe { shared_mem.boxed::<MyType>() };
```
call to this function is inherently unsafe since there is no guarantee that the memory behind the pointer is initialized or valid.

```rust
type NotZeroI32 = i32;

let boxed_val = unsafe { 
  let mut boxed_val = shared_mem.boxed::<NotZeroI32>();
  *boxed_val = 5;
  };
```
the `ShmemBox` type implements `Deref` and `DerefMut` so you can use all the rust semantics and guarantee of `T` in your code

When the variable goes out of scope, the `drop` implementation is called. if the shared memory is owned, the shared memory would unlink.
in order to prevent this, you can use the `ShmemBox::leak` method.

```rust

struct MyType;

impl Drop for MyType{
    fn drop(&mut self) {
        println!("my type is dropped");
    }
}

{
  let shared_mem = shmem::Builder::new("<FLINK_FILE_HANDLE>")
      .with_size(mem::size_of::<Message>() as i64)
      .open()?;
  let mut boxed_val = unsafe {shared_mem.boxed::<MyType>()};

  // boxed_val goes out of scope and the underlying MyType is dropped
  // output:
  // my type is dropped
}
{
  let shared_mem = shmem::Builder::new("<FLINK_FILE_HANDLE>")
      .with_size(mem::size_of::<Message>() as i64)
      .open()?;
  let mut boxed_val = unsafe {shared_mem.boxed::<MyType>()};
  ShmemBox::leak(boxed_val);

  // boxed_val leaks and the underlying MyType is not dropped. the shared memory stays linked to the os
  // output is empty
}
```
you can also use the `ShmemBox::own` to ensure cleanup of the shared memory 
```rust 
struct MyType;

impl Drop for MyType{
    fn drop(&mut self) {
        println!("my type is dropped");
    }
}

{
  let shared_mem = shmem::Builder::new("<FLINK_FILE_HANDLE>")
      .with_size(mem::size_of::<Message>() as i64)
      .open()?;
  let mut boxed_val = unsafe {shared_mem.boxed::<MyType>()};
  
  // boxed_val was owner, but it got leaked
  ShmemBox::leak(boxed_val);
}
{
  let shared_mem = shmem::Builder::new("<FLINK_FILE_HANDLE>")
      .with_size(mem::size_of::<Message>() as i64)
      .open()?;
  let mut boxed_val = unsafe {shared_mem.boxed::<MyType>()};
  
  // shared memory is already created, so the boxed_val is not the owner.
  let boxed_val = ShmemBox::own(boxed_val);

  // boxed_val goes out of scope, MyType is dropped. the shared memory is unliked.
  // output:
  // my type is dropped
}
```

`ShmemBox<T>` implements `Sync` and `Send` if the underlying `T` implements `Sync` and `Send` respectively
