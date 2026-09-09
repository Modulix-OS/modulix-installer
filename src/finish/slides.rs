use crate::widgets::slideshow::Slide;

/// Shown by [`crate::finish::progress::ProgressPage`] while the pipeline
/// runs. Placeholders — swap the title/body msgids and the SVG resource for
/// real content later; adding a slide is one more entry here plus one SVG
/// registered in `data/resources.gresource.xml`.
pub const INSTALL_SLIDES: &[Slide] = &[
    Slide {
        resource: "/org/modulix/installer/images/slides/slide-1.svg",
        title: "Welcome to Modulix OS",
        body: "We're setting things up in the background — this won't take long.",
    },
    Slide {
        resource: "/org/modulix/installer/images/slides/slide-2.svg",
        title: "A modular system",
        body: "Modulix OS is built on NixOS, so your configuration stays reproducible.",
    },
    Slide {
        resource: "/org/modulix/installer/images/slides/slide-3.svg",
        title: "Your choices, applied",
        body: "Partitioning, encryption, and your desktop environment are being put in place.",
    },
    Slide {
        resource: "/org/modulix/installer/images/slides/slide-4.svg",
        title: "Almost there",
        body: "Once this finishes, you'll be ready to restart into your new system.",
    },
];
