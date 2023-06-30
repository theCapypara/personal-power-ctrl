use crate::settings::{SinkBaseSettings, SourceBaseSettings};
use crate::sink::Sink;
use crate::source::Source;
use std::borrow::Cow;
use std::fmt::{Display, Formatter};
use std::ops::Deref;

#[repr(transparent)]
/// Transparent newtype wrapper for a type that is only a Sink, not a source.
pub struct IsSink(pub Box<dyn Sink>);
impl Deref for IsSink {
    type Target = dyn Sink;

    fn deref(&self) -> &Self::Target {
        &*self.0
    }
}

#[repr(transparent)]
/// Transparent newtype wrapper for a type that is only a Source, not a sink.
pub struct IsSource(pub Box<dyn Source>);
impl Deref for IsSource {
    type Target = dyn Source;

    fn deref(&self) -> &Self::Target {
        &*self.0
    }
}

#[derive(Hash, Debug, Clone, PartialEq, Eq)]
/// The identity of a named object.
pub struct Identity<'a> {
    category: &'static str,
    name: Cow<'a, str>,
}

impl<'a> Display for Identity<'a> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{:6}] [{}]", self.category, self.name)
    }
}

impl<'a> Identity<'a> {
    pub fn clone_owned(&self) -> Identity<'static> {
        Identity {
            category: self.category,
            name: Cow::Owned(self.name.clone().into_owned()),
        }
    }
}

/// Something that has a name and a category for the purposes of categorization and logging.
pub trait Named {
    fn category(&self) -> &'static str;
    fn name(&self) -> &str;
    fn identity(&self) -> Identity {
        Identity {
            category: self.category(),
            name: Cow::Borrowed(self.name()),
        }
    }
}

impl Named for SinkBaseSettings {
    fn category(&self) -> &'static str {
        "sink"
    }
    fn name(&self) -> &str {
        &self.name
    }
}

impl Named for SourceBaseSettings {
    fn category(&self) -> &'static str {
        "source"
    }
    fn name(&self) -> &str {
        &self.name
    }
}

impl Named for IsSink {
    #[inline]
    fn category(&self) -> &'static str {
        self.base_settings().category()
    }
    #[inline]
    fn name(&self) -> &str {
        self.base_settings().name()
    }
}

impl Named for IsSource {
    #[inline]
    fn category(&self) -> &'static str {
        self.base_settings().category()
    }
    #[inline]
    fn name(&self) -> &str {
        self.base_settings().name()
    }
}
