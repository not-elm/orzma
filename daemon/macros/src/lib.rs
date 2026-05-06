/// Generates a tuple struct wrapping `String` with `Debug`, `Clone`,
/// `Eq`/`PartialEq`, `Hash`, `Serialize`/`Deserialize`, `AsRef<str>`,
/// `Display`, and a `new()` constructor returning a v4 UUID string.
///
/// # Caller-side dependencies
///
/// Path resolution for declarative macros happens at the call site, so
/// every crate that invokes this macro must list these direct deps in
/// its own `Cargo.toml`:
///
/// - `serde` (with the `derive` feature)
/// - `uuid` (with the `v4` feature)
#[macro_export]
macro_rules! define_string_new_type {
    ($name:ident) => {
        #[derive(Debug, Clone, Eq, PartialEq, serde::Serialize, serde::Deserialize, Hash)]
        pub struct $name(String);

        impl $name {
            /// Creates a new unique identifier.
            pub fn new() -> Self {
                Self(uuid::Uuid::new_v4().to_string())
            }
        }

        impl AsRef<str> for $name {
            #[inline]
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}
