//! kstring 2.0.4 deprecated `KString`, `KStringCow` and `KStringRef` in favour of
//! the string-rosetta-rs alternatives, but liquid's public traits are still
//! written in terms of them, so there is nothing to migrate to. Aliasing them
//! here keeps the `deprecated` allow in one place instead of in every module that
//! implements a liquid trait.

#![allow(deprecated)]

pub(crate) type KString = liquid_core::model::KString;
pub(crate) type KStringCow<'s> = liquid_core::model::KStringCow<'s>;
pub(crate) type KStringRef<'s> = liquid_core::model::KStringRef<'s>;
