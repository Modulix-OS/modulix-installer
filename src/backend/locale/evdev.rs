use super::{KeyboardLayout, KeyboardVariant};
use quick_xml::Reader;
use quick_xml::escape::unescape;
use quick_xml::events::Event;

#[derive(Default)]
struct LayoutBuilder {
    code: String,
    description: String,
    variants: Vec<KeyboardVariant>,
}

#[derive(Default)]
struct VariantBuilder {
    code: String,
    description: String,
}

/// Parses xkeyboard-config's `rules/evdev.xml` (`<layoutList><layout>
/// <configItem><name>/<description></configItem><variantList><variant>...`).
/// Malformed input yields an empty/partial result rather than erroring —
/// this only ever feeds a UI dropdown.
pub fn parse_evdev_xml(xml: &str) -> Vec<KeyboardLayout> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut layouts = Vec::new();
    let mut current_layout: Option<LayoutBuilder> = None;
    let mut current_variant: Option<VariantBuilder> = None;
    let mut in_variant_list = false;
    let mut tag_stack: Vec<String> = Vec::new();
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                match name.as_str() {
                    "layout" => {
                        current_layout = Some(LayoutBuilder::default());
                        in_variant_list = false;
                    }
                    "variantList" => in_variant_list = true,
                    "variant" => current_variant = Some(VariantBuilder::default()),
                    _ => {}
                }
                tag_stack.push(name);
            }
            Ok(Event::Text(t)) => {
                let text = std::str::from_utf8(&t)
                    .ok()
                    .and_then(|s| unescape(s).ok())
                    .map(|s| s.into_owned())
                    .unwrap_or_default();
                if let Some(tag) = tag_stack.last().map(String::as_str) {
                    if let Some(variant) = current_variant.as_mut() {
                        match tag {
                            "name" => variant.code = text,
                            "description" => variant.description = text,
                            _ => {}
                        }
                    } else if !in_variant_list && let Some(layout) = current_layout.as_mut() {
                        match tag {
                            "name" => layout.code = text,
                            "description" => layout.description = text,
                            _ => {}
                        }
                    }
                }
            }
            Ok(Event::End(e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                tag_stack.pop();
                match name.as_str() {
                    "variant" => {
                        if let (Some(layout), Some(variant)) =
                            (current_layout.as_mut(), current_variant.take())
                        {
                            layout.variants.push(KeyboardVariant {
                                code: variant.code,
                                description: variant.description,
                            });
                        }
                    }
                    "variantList" => in_variant_list = false,
                    "layout" => {
                        if let Some(layout) = current_layout.take() {
                            layouts.push(KeyboardLayout {
                                code: layout.code,
                                description: layout.description,
                                variants: layout.variants,
                            });
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    layouts
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<xkbConfigRegistry version="1.1">
  <layoutList>
    <layout>
      <configItem>
        <name>us</name>
        <description>English (US)</description>
      </configItem>
    </layout>
    <layout>
      <configItem>
        <name>fr</name>
        <description>French</description>
      </configItem>
      <variantList>
        <variant>
          <configItem>
            <name>oss</name>
            <description>French (alt.)</description>
          </configItem>
        </variant>
        <variant>
          <configItem>
            <name>bepo</name>
            <description>French (BEPO)</description>
          </configItem>
        </variant>
      </variantList>
    </layout>
  </layoutList>
  <optionList>
    <group>
      <configItem>
        <name>grp</name>
        <description>Switching to another layout</description>
      </configItem>
    </group>
  </optionList>
</xkbConfigRegistry>
"#;

    #[test]
    fn parses_layouts_and_variants() {
        let layouts = parse_evdev_xml(SAMPLE);
        assert_eq!(layouts.len(), 2);

        assert_eq!(layouts[0].code, "us");
        assert_eq!(layouts[0].description, "English (US)");
        assert!(layouts[0].variants.is_empty());

        assert_eq!(layouts[1].code, "fr");
        assert_eq!(layouts[1].variants.len(), 2);
        assert_eq!(layouts[1].variants[0].code, "oss");
        assert_eq!(layouts[1].variants[1].code, "bepo");
    }

    #[test]
    fn ignores_option_list() {
        let layouts = parse_evdev_xml(SAMPLE);
        assert!(!layouts.iter().any(|l| l.code == "grp"));
    }

    #[test]
    fn empty_input_yields_no_layouts() {
        assert!(parse_evdev_xml("").is_empty());
    }
}
