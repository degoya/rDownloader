/// Declares a stable UUIDv7 identifier type.
///
/// Exported for `rd-core`, which declares the service's own identifiers with it; only the three a
/// plugin names live here. The calling crate needs `serde`, `utoipa` and `uuid` itself.
#[doc(hidden)]
#[macro_export]
macro_rules! domain_id {
    ($name:ident) => {
        #[doc = concat!("Stable UUIDv7 identifier for ", stringify!($name), ".")]
        #[derive(
            Clone,
            Copy,
            Debug,
            ::serde::Deserialize,
            Eq,
            Hash,
            Ord,
            PartialEq,
            PartialOrd,
            ::serde::Serialize,
            ::utoipa::ToSchema,
        )]
        #[serde(transparent)]
        #[schema(value_type = String, format = Uuid)]
        pub struct $name(::uuid::Uuid);

        impl $name {
            /// Creates a time-ordered UUIDv7 identifier.
            #[must_use]
            pub fn new() -> Self {
                Self(::uuid::Uuid::now_v7())
            }

            /// Wraps an existing UUID.
            #[must_use]
            pub const fn from_uuid(value: ::uuid::Uuid) -> Self {
                Self(value)
            }

            /// Returns the underlying UUID.
            #[must_use]
            pub const fn into_uuid(self) -> ::uuid::Uuid {
                self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl ::std::fmt::Display for $name {
            fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                self.0.fmt(formatter)
            }
        }

        impl ::std::str::FromStr for $name {
            type Err = ::uuid::Error;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                ::uuid::Uuid::parse_str(value).map(Self)
            }
        }
    };
}

domain_id!(AccountId);
domain_id!(PluginId);
domain_id!(ProxyProfileId);
