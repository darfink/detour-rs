//! Links the C library, which is otherwise linked by `std`.
fn main() {
  let library = match std::env::var("CARGO_CFG_TARGET_VENDOR").as_deref() {
    Ok("apple") => "System",
    _ => "c",
  };
  println!("cargo:rustc-link-lib={library}");
}
