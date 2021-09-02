use std::convert::{TryFrom, TryInto};
use std::ffi::{OsStr, OsString};
use std::fmt::Display;
use std::str::FromStr;

use eyre::Context;

use crate::git::repo::wrap_git_error;

/// Represents the ID of a Git object.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NonZeroOid {
    pub(super) inner: git2::Oid,
}

impl NonZeroOid {
    /// Convert this OID into its raw 20-byte slice.
    pub fn as_bytes(&self) -> &[u8] {
        self.inner.as_bytes()
    }
}

impl std::fmt::Debug for NonZeroOid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "NonZeroOid({:?})", self.inner)
    }
}

impl Display for NonZeroOid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self.inner)
    }
}

impl TryFrom<MaybeZeroOid> for NonZeroOid {
    type Error = eyre::Error;

    fn try_from(value: MaybeZeroOid) -> Result<Self, Self::Error> {
        match value {
            MaybeZeroOid::NonZero(non_zero_oid) => Ok(non_zero_oid),
            MaybeZeroOid::Zero => eyre::bail!("Expected a non-zero OID"),
        }
    }
}

impl TryFrom<OsString> for NonZeroOid {
    type Error = eyre::Error;

    fn try_from(value: OsString) -> Result<Self, Self::Error> {
        let value: &OsStr = &value;
        value.try_into()
    }
}

impl TryFrom<&OsStr> for NonZeroOid {
    type Error = eyre::Error;

    fn try_from(value: &OsStr) -> Result<Self, Self::Error> {
        let oid: MaybeZeroOid = value.try_into()?;
        match oid {
            MaybeZeroOid::Zero => eyre::bail!("OID was zero, but expected to be non-zero"),
            MaybeZeroOid::NonZero(oid) => Ok(oid),
        }
    }
}

impl FromStr for NonZeroOid {
    type Err = eyre::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let oid: MaybeZeroOid = value.parse()?;
        match oid {
            MaybeZeroOid::NonZero(non_zero_oid) => Ok(non_zero_oid),
            MaybeZeroOid::Zero => eyre::bail!("Expected a non-zero OID, but got: {:?}", value),
        }
    }
}

pub(super) fn make_non_zero_oid(oid: git2::Oid) -> NonZeroOid {
    assert_ne!(oid, git2::Oid::zero());
    NonZeroOid { inner: oid }
}

/// Represents an OID which may be zero or non-zero. This exists because Git
/// often represents the absence of an object using the zero OID. We want to
/// statically check for those cases by using a more descriptive type.
///
/// This type is isomorphic to `Option<NonZeroOid>`. It should be used primarily
/// when converting to and from string representations of OID values.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MaybeZeroOid {
    /// The zero OID (i.e. 40 `0`s).
    Zero,

    /// A non-zero OID.
    NonZero(NonZeroOid),
}

impl MaybeZeroOid {
    /// Construct an OID from a raw 20-byte slice.
    pub fn from_bytes(bytes: &[u8]) -> eyre::Result<Self> {
        let oid = git2::Oid::from_bytes(bytes)?;
        Ok(oid.into())
    }
}

impl std::fmt::Debug for MaybeZeroOid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self)
    }
}

impl Display for MaybeZeroOid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let zero = git2::Oid::zero();
        write!(
            f,
            "{:?}",
            match self {
                MaybeZeroOid::NonZero(NonZeroOid { inner }) => inner,
                MaybeZeroOid::Zero => &zero,
            }
        )
    }
}

impl FromStr for MaybeZeroOid {
    type Err = eyre::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.parse() {
            Ok(oid) if oid == git2::Oid::zero() => Ok(MaybeZeroOid::Zero),
            Ok(oid) => Ok(MaybeZeroOid::NonZero(NonZeroOid { inner: oid })),
            Err(err) => Err(wrap_git_error(err))
                .wrap_err_with(|| format!("Could not parse OID from string: {:?}", s)),
        }
    }
}

impl From<git2::Oid> for MaybeZeroOid {
    fn from(oid: git2::Oid) -> Self {
        if oid.is_zero() {
            Self::Zero
        } else {
            Self::NonZero(make_non_zero_oid(oid))
        }
    }
}

impl TryFrom<&OsStr> for MaybeZeroOid {
    type Error = eyre::Error;

    fn try_from(value: &OsStr) -> Result<Self, Self::Error> {
        match value.to_str() {
            None => eyre::bail!("OID value was not a simple ASCII value: {:?}", value),
            Some(value) => value.parse(),
        }
    }
}

impl TryFrom<OsString> for MaybeZeroOid {
    type Error = eyre::Error;

    fn try_from(value: OsString) -> Result<Self, Self::Error> {
        MaybeZeroOid::try_from(value.as_os_str())
    }
}

impl From<NonZeroOid> for MaybeZeroOid {
    fn from(oid: NonZeroOid) -> Self {
        Self::NonZero(oid)
    }
}

impl From<Option<NonZeroOid>> for MaybeZeroOid {
    fn from(oid: Option<NonZeroOid>) -> Self {
        match oid {
            Some(oid) => MaybeZeroOid::NonZero(oid),
            None => MaybeZeroOid::Zero,
        }
    }
}

impl From<MaybeZeroOid> for Option<NonZeroOid> {
    fn from(oid: MaybeZeroOid) -> Self {
        match oid {
            MaybeZeroOid::Zero => None,
            MaybeZeroOid::NonZero(oid) => Some(oid),
        }
    }
}
