use region;
use error::*;

/// Returns true if the address is executable
pub fn is_executable_address(address: *const ()) -> Result<bool> {
    Ok(region::query(address as *const _)?.protection.contains(region::Protection::Execute))
}

/// Returns true if the displacement is within 2GB.
pub fn is_within_2gb(displacement: isize) -> bool {
    let range = (i32::min_value() as isize)..(i32::max_value() as isize);
    range.contains(displacement)
}
