pub mod app;
pub mod commands;
pub mod config;
pub mod devices;
pub mod engine;
pub mod input;
pub mod link;
pub mod midi;
pub mod music;
pub mod pattern;
pub mod persist;
pub mod ui;

/// Minimal test-only constructors shared across inline unit tests.
#[cfg(test)]
pub(crate) mod test_support {
    use crate::app::App;
    use crate::devices::profiles::default_profiles;
    use crate::pattern::library::Library;
    use crate::pattern::model::Set;

    /// Build an `App` the way production does in `main.rs`: a default `Set`
    /// over the default device profiles and an empty `Library`.
    pub(crate) fn app_for_tests() -> App {
        App::new(Set::default_set(default_profiles()), Library::empty())
    }
}
