//! State hashing: xxh3 over the canonical postcard encoding.
//!
//! Postcard is canonical here because every hashed container iterates in a
//! total order (`BTreeMap`, `Vec` in id order), so equal states always
//! produce equal bytes.

use serde::Serialize;

/// xxh3 64-bit hash of raw bytes.
pub fn hash_bytes(bytes: &[u8]) -> u64 {
    xxhash_rust::xxh3::xxh3_64(bytes)
}

/// xxh3 64-bit hash of the postcard encoding of `value`.
pub fn hash_value<T: Serialize + ?Sized>(value: &T) -> u64 {
    let bytes = postcard::to_allocvec(value).expect("hashed state serialises");
    hash_bytes(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_values_hash_equal_and_differ_on_change() {
        assert_eq!(hash_value(&(1u32, 2u32)), hash_value(&(1u32, 2u32)));
        assert_ne!(hash_value(&(1u32, 2u32)), hash_value(&(2u32, 1u32)));
        assert_eq!(hash_bytes(b"eonmark"), hash_bytes(b"eonmark"));
    }
}
