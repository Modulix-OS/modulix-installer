//! `StepValidity` as a GObject property, so the "Next" button can
//! `bind_property("ready", next_button, "sensitive")` and update itself with
//! zero polling.

glib::wrapper! {
    pub struct ValidityTracker(ObjectSubclass<imp::ValidityTracker>);
}

impl ValidityTracker {
    pub fn ready() -> Self {
        let tracker: Self = glib::Object::new();
        tracker.set_ready();
        tracker
    }

    pub fn blocked(reason: impl Into<String>) -> Self {
        let tracker: Self = glib::Object::new();
        tracker.set_blocked(reason);
        tracker
    }

    pub fn set_ready(&self) {
        self.set_is_ready(true);
        self.set_reason(String::new());
    }

    pub fn set_blocked(&self, reason: impl Into<String>) {
        self.set_is_ready(false);
        self.set_reason(reason.into());
    }
}

impl Default for ValidityTracker {
    fn default() -> Self {
        Self::ready()
    }
}

mod imp {
    use glib::prelude::*;
    use glib::subclass::prelude::*;
    use std::cell::{Cell, RefCell};

    #[derive(glib::Properties, Default)]
    #[properties(wrapper_type = super::ValidityTracker)]
    pub struct ValidityTracker {
        #[property(get, set, name = "is-ready")]
        is_ready: Cell<bool>,
        #[property(get, set)]
        reason: RefCell<String>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ValidityTracker {
        const NAME: &'static str = "ModulixValidityTracker";
        type Type = super::ValidityTracker;
    }

    #[glib::derived_properties]
    impl ObjectImpl for ValidityTracker {}
}
