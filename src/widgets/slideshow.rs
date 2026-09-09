//! Auto-advancing carousel of explainer slides, used by the install-progress
//! page to fill the wait with content instead of a bare log. Reusable: adding
//! a slide is one SVG + one gresource line + one table entry (see
//! `crate::finish::slides`).

const AUTO_ADVANCE_SECONDS: u32 = 8;

/// One slide: a pictogram (gresource path) + two gettext msgids.
pub struct Slide {
    pub resource: &'static str,
    pub title: &'static str,
    pub body: &'static str,
}

/// Arms a one-shot timer that advances `carousel` by one page (looping) and
/// then re-arms itself via `connect_page_changed` on the way back in
/// (see [`Slideshow::new`]) — as long as `running` still says so.
fn arm_timer(
    carousel: &adw::Carousel,
    timer: &std::rc::Rc<std::cell::Cell<Option<glib::SourceId>>>,
    running: &std::rc::Rc<std::cell::Cell<bool>>,
) {
    use adw::prelude::*;
    if let Some(old) = timer.take() {
        old.remove();
    }
    let carousel_weak = carousel.downgrade();
    let running = running.clone();
    let id = glib::timeout_add_seconds_local(AUTO_ADVANCE_SECONDS, move || {
        if !running.get() {
            return glib::ControlFlow::Break;
        }
        let Some(carousel) = carousel_weak.upgrade() else {
            return glib::ControlFlow::Break;
        };
        let n = carousel.n_pages();
        if n > 0 {
            let next = (carousel.position().round() as u32 + 1) % n;
            carousel.scroll_to(&carousel.nth_page(next), true);
        }
        glib::ControlFlow::Break
    });
    timer.set(Some(id));
}

pub struct Slideshow {
    widget: gtk::Widget,
    carousel: adw::Carousel,
    slides: &'static [Slide],
    labels: Vec<(gtk::Label, gtk::Label)>,
    timer: std::rc::Rc<std::cell::Cell<Option<glib::SourceId>>>,
    running: std::rc::Rc<std::cell::Cell<bool>>,
}

impl Slideshow {
    pub fn new(slides: &'static [Slide]) -> Self {
        use crate::i18n::tr;
        use adw::prelude::*;

        let carousel = adw::Carousel::builder()
            .allow_scroll_wheel(true)
            .allow_mouse_drag(true)
            .vexpand(true)
            .build();

        let mut labels = Vec::with_capacity(slides.len());
        for slide in slides {
            let picture = gtk::Picture::for_resource(slide.resource);
            picture.set_content_fit(gtk::ContentFit::Contain);
            picture.set_can_shrink(true);
            let frame = gtk::AspectFrame::builder()
                .ratio(16.0 / 9.0)
                .obey_child(false)
                .child(&picture)
                .build();

            let title_label = gtk::Label::builder()
                .label(tr(slide.title))
                .css_classes(["title-2"])
                .justify(gtk::Justification::Center)
                .wrap(true)
                .build();
            let body_label = gtk::Label::builder()
                .label(tr(slide.body))
                .css_classes(["dim-label"])
                .justify(gtk::Justification::Center)
                .wrap(true)
                .build();

            let page = gtk::Box::new(gtk::Orientation::Vertical, 12);
            page.set_valign(gtk::Align::Center);
            page.append(&frame);
            page.append(&title_label);
            page.append(&body_label);

            carousel.append(&page);
            labels.push((title_label, body_label));
        }

        let dots = adw::CarouselIndicatorDots::builder()
            .carousel(&carousel)
            .halign(gtk::Align::Center)
            .build();
        dots.set_visible(slides.len() > 1);

        let outer = gtk::Box::new(gtk::Orientation::Vertical, 12);
        outer.append(&carousel);
        outer.append(&dots);

        let timer: std::rc::Rc<std::cell::Cell<Option<glib::SourceId>>> =
            std::rc::Rc::new(std::cell::Cell::new(None));
        let running = std::rc::Rc::new(std::cell::Cell::new(false));

        {
            let timer = timer.clone();
            let running = running.clone();
            let carousel_weak = carousel.downgrade();
            carousel.connect_page_changed(move |_carousel, _idx| {
                if let Some(old) = timer.take() {
                    old.remove();
                }
                // Any page change — manual or auto — re-arms the timer, so a
                // swipe/click/scroll pushes the next auto-advance out by a
                // full interval instead of firing right away.
                if running.get()
                    && let Some(carousel) = carousel_weak.upgrade()
                {
                    arm_timer(&carousel, &timer, &running);
                }
            });
        }

        Self {
            widget: outer.upcast(),
            carousel,
            slides,
            labels,
            timer,
            running,
        }
    }

    pub fn widget(&self) -> gtk::Widget {
        self.widget.clone()
    }

    /// Arms the 8s auto-advance timer. Idempotent: calling it again just
    /// replaces the pending timer.
    pub fn start(&self) {
        if self.slides.len() <= 1 {
            return;
        }
        self.running.set(true);
        arm_timer(&self.carousel, &self.timer, &self.running);
    }

    /// Desarms the auto-advance timer — call when the installation finishes
    /// or the page is left.
    pub fn stop(&self) {
        self.running.set(false);
        if let Some(old) = self.timer.take() {
            old.remove();
        }
    }

    pub fn retranslate(&self) {
        use crate::i18n::tr;
        for (slide, (title_label, body_label)) in self.slides.iter().zip(self.labels.iter()) {
            title_label.set_label(&tr(slide.title));
            body_label.set_label(&tr(slide.body));
        }
    }
}
