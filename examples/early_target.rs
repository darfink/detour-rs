//! An ordinary program, used as the target of the `early_hook` example.
//!
//! It queries its process ID first thing in `main`; the `early_hook` library
//! intercepts that call, demonstrating that no invocation is missed.

fn main() {
  println!("process id: {}", std::process::id());
}
