#[macro_export]
macro_rules! define_string_new_type {
    ($name: ident) => {
        #[derive(Debug, Clone, Eq, PartialEq, serde::Serialize, serde::Deserialize, Hash)]
        pub struct $name(String);

        impl $name {
            /// Create the new session-id with a unique identifier
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
