// Copied from auto_array 0.1.1
// https://crates.io/crates/auto_array/0.1.1/code/src/lib.rs
macro_rules! auto_array {
    ($($(#[$attr:meta])* $vis:vis $const_or_static:ident $name:ident: [$ty:ty; _] = $array:expr;)*) => {
        $($(#[$attr])* $vis $const_or_static $name: [$ty; <[$ty]>::len(&$array)] = $array;)*
    };
}
pub(crate) use auto_array;
