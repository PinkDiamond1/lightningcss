use crate::values::string::CowArcStr;
use cssparser::*;
use smallvec::SmallVec;
use crate::dependencies::{UrlDependency, Dependency};
use crate::vendor_prefix::VendorPrefix;
use crate::prefixes::{Feature, is_webkit_gradient};
use crate::targets::Browsers;
use crate::traits::{Parse, ToCss, FallbackValues};
use crate::printer::Printer;
use super::color::ColorFallbackKind;
use super::gradient::*;
use super::resolution::Resolution;
use crate::values::url::Url;
use crate::error::{ParserError, PrinterError};

/// https://www.w3.org/TR/css-images-3/#typedef-image
#[derive(Debug, Clone, PartialEq)]
pub enum Image<'i> {
  None,
  Url(Url<'i>),
  Gradient(Box<Gradient>),
  ImageSet(ImageSet<'i>)
}

impl<'i> Default for Image<'i> {
  fn default() -> Image<'i> {
    Image::None
  }
}

impl<'i> Image<'i> {
  pub fn has_vendor_prefix(&self) -> bool {
    let prefix = self.get_vendor_prefix();
    !prefix.is_empty() && prefix != VendorPrefix::None
  }

  pub fn get_vendor_prefix(&self) -> VendorPrefix {
    match self {
      Image::Gradient(a) => a.get_vendor_prefix(),
      Image::ImageSet(a) => a.get_vendor_prefix(),
      _ => VendorPrefix::empty()
    }
  }

  pub fn get_necessary_prefixes(&self, targets: Browsers) -> VendorPrefix {
    match self {
      Image::Gradient(grad) => grad.get_necessary_prefixes(targets),
      Image::ImageSet(image_set) => image_set.get_necessary_prefixes(targets),
      _ => VendorPrefix::None
    }
  }

  pub fn get_prefixed(&self, prefix: VendorPrefix) -> Image<'i> {
    match self {
      Image::Gradient(grad) => Image::Gradient(Box::new(grad.get_prefixed(prefix))),
      Image::ImageSet(image_set) => Image::ImageSet(image_set.get_prefixed(prefix)),
      _ => self.clone()
    }
  }

  pub fn get_legacy_webkit(&self) -> Result<Image<'i>, ()> {
    match self {
      Image::Gradient(grad) => Ok(Image::Gradient(Box::new(grad.get_legacy_webkit()?))),
      _ => Ok(self.clone())
    }
  }

  pub fn get_necessary_fallbacks(&self, targets: Browsers) -> ColorFallbackKind {
    match self {
      Image::Gradient(grad) => grad.get_necessary_fallbacks(targets),
      _ => ColorFallbackKind::empty()
    }
  }

  pub fn get_fallback(&self, kind: ColorFallbackKind) -> Image<'i> {
    match self {
      Image::Gradient(grad) => Image::Gradient(Box::new(grad.get_fallback(kind))),
      _ => self.clone()
    }
  }
}

pub(crate) trait ImageFallback<'i>: Sized {
  fn get_image(&self) -> &Image<'i>;
  fn with_image(&self, image: Image<'i>) -> Self;

  #[inline]
  fn get_necessary_fallbacks(&self, targets: Browsers) -> ColorFallbackKind {
    self.get_image().get_necessary_fallbacks(targets)
  }

  #[inline]
  fn get_fallback(&self, kind: ColorFallbackKind) -> Self {
    self.with_image(self.get_image().get_fallback(kind))
  }
}

impl<'i> ImageFallback<'i> for Image<'i> {
  #[inline]
  fn get_image(&self) -> &Image<'i> {
    self
  }

  #[inline]
  fn with_image(&self, image: Image<'i>) -> Self {
    image
  }
}

impl<'i> FallbackValues for Image<'i> {
  fn get_fallbacks(&mut self, targets: Browsers) -> Vec<Self> {
    // Determine which prefixes and color fallbacks are needed.
    let prefixes = self.get_necessary_prefixes(targets);
    let fallbacks = self.get_necessary_fallbacks(targets);
    let mut res = Vec::new();

    // Get RGB fallbacks if needed.
    let rgb = if fallbacks.contains(ColorFallbackKind::RGB) {
      Some(self.get_fallback(ColorFallbackKind::RGB))
    } else {
      None
    };

    // Prefixed properties only support RGB.
    let prefix_image = rgb.as_ref().unwrap_or(self);

    // Legacy -webkit-gradient()
    if prefixes.contains(VendorPrefix::WebKit) && is_webkit_gradient(targets) && matches!(prefix_image, Image::Gradient(_)) {
      if let Ok(legacy) = prefix_image.get_legacy_webkit() {
        res.push(legacy);
      }
    }

    // Standard syntax, with prefixes.
    if prefixes.contains(VendorPrefix::WebKit) {
      res.push(prefix_image.get_prefixed(VendorPrefix::WebKit))
    }

    if prefixes.contains(VendorPrefix::Moz) {
      res.push(prefix_image.get_prefixed(VendorPrefix::Moz))
    }

    if prefixes.contains(VendorPrefix::O) {
      res.push(prefix_image.get_prefixed(VendorPrefix::O))
    }

    if prefixes.contains(VendorPrefix::None) {
      // Unprefixed, rgb fallback.
      if let Some(rgb) = rgb {
        res.push(rgb);
      }

      // P3 fallback.
      if fallbacks.contains(ColorFallbackKind::P3) {
        res.push(self.get_fallback(ColorFallbackKind::P3));
      }

      // Convert original to lab if needed (e.g. if oklab is not supported but lab is).
      if fallbacks.contains(ColorFallbackKind::LAB) {
        *self = self.get_fallback(ColorFallbackKind::LAB);
      }
    } else if let Some(last) = res.pop() {
      // Prefixed property with no unprefixed version.
      // Replace self with the last prefixed version so that it doesn't
      // get duplicated when the caller pushes the original value.
      *self = last;
    }

    res
  }
}

impl<'i, T: ImageFallback<'i>> FallbackValues for SmallVec<[T; 1]> {
  fn get_fallbacks(&mut self, targets: Browsers) -> Vec<Self> {
    // Determine what vendor prefixes and color fallbacks are needed.
    let mut prefixes = VendorPrefix::empty();
    let mut fallbacks = ColorFallbackKind::empty();
    let mut res = Vec::new();
    for item in self.iter() {
      prefixes |= item.get_image().get_necessary_prefixes(targets);
      fallbacks |= item.get_necessary_fallbacks(targets);
    }

    // Get RGB fallbacks if needed.
    let rgb: Option<SmallVec<[T; 1]>> = if fallbacks.contains(ColorFallbackKind::RGB) {
      Some(self
        .iter()
        .map(|item| item.get_fallback(ColorFallbackKind::RGB))
        .collect())
    } else {
      None
    };

    // Prefixed properties only support RGB.
    let prefix_images = rgb.as_ref().unwrap_or(&self);
  
    // Legacy -webkit-gradient()
    if prefixes.contains(VendorPrefix::WebKit) && is_webkit_gradient(targets) {
      let images: SmallVec<[T; 1]> = prefix_images.iter().map(|item| {
        item
          .get_image()
          .get_legacy_webkit()
          .map(|image| item.with_image(image))
      }).flatten().collect();
      if !images.is_empty() {
        res.push(images)
      }
    }

    // Standard syntax, with prefixes.
    macro_rules! prefix {
      ($prefix: ident) => {
        if prefixes.contains(VendorPrefix::$prefix) {
          let images = prefix_images.iter().map(|item| {
            let image = item.get_image().get_prefixed(VendorPrefix::$prefix);
            item.with_image(image)
          }).collect();
          res.push(images)
        }
      };
    }

    prefix!(WebKit);
    prefix!(Moz);
    prefix!(O);
    if prefixes.contains(VendorPrefix::None) {
      if let Some(rgb) = rgb {
        res.push(rgb);
      }

      if fallbacks.contains(ColorFallbackKind::P3) {
        let p3_images = self
          .iter()
          .map(|item| item.get_fallback(ColorFallbackKind::P3))
          .collect();

        res.push(p3_images)
      }

      // Convert to lab if needed (e.g. if oklab is not supported but lab is).
      if fallbacks.contains(ColorFallbackKind::LAB) {
        for item in self.iter_mut() {
          *item = item.get_fallback(ColorFallbackKind::LAB);
        }
      }
    } else if let Some(last) = res.pop() {
      // Prefixed property with no unprefixed version.
      // Replace self with the last prefixed version so that it doesn't
      // get duplicated when the caller pushes the original value.
      *self = last;
    }

    res
  }
}

impl<'i> Parse<'i> for Image<'i> {
  fn parse<'t>(input: &mut Parser<'i, 't>) -> Result<Self, ParseError<'i, ParserError<'i>>> {
    if input.try_parse(|i| i.expect_ident_matching("none")).is_ok() {
      return Ok(Image::None)
    }
    
    if let Ok(url) = input.try_parse(Url::parse) {
      return Ok(Image::Url(url))
    }

    if let Ok(grad) = input.try_parse(Gradient::parse) {
      return Ok(Image::Gradient(Box::new(grad)))
    }

    if let Ok(image_set) = input.try_parse(ImageSet::parse) {
      return Ok(Image::ImageSet(image_set))
    }

    Err(input.new_error_for_next_token())
  }
}

impl<'i> ToCss for Image<'i> {
  fn to_css<W>(&self, dest: &mut Printer<W>) -> Result<(), PrinterError> where W: std::fmt::Write {
    match self {
      Image::None => dest.write_str("none"),
      Image::Url(url) => url.to_css(dest),
      Image::Gradient(grad) => grad.to_css(dest),
      Image::ImageSet(image_set) => image_set.to_css(dest)
    }
  }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImageSet<'i> {
  pub options: Vec<ImageSetOption<'i>>,
  pub vendor_prefix: VendorPrefix
}

impl<'i> ImageSet<'i> {
  pub fn get_vendor_prefix(&self) -> VendorPrefix {
    self.vendor_prefix
  }

  pub fn get_necessary_prefixes(&self, targets: Browsers) -> VendorPrefix {
    if self.vendor_prefix.contains(VendorPrefix::None) {
      Feature::ImageSet.prefixes_for(targets)
    } else {
      self.vendor_prefix
    }
  }

  pub fn get_prefixed(&self, prefix: VendorPrefix) -> ImageSet<'i> {
    ImageSet {
      options: self.options.clone(),
      vendor_prefix: prefix
    }
  }
}

impl<'i> Parse<'i> for ImageSet<'i> {
  fn parse<'t>(input: &mut Parser<'i, 't>) -> Result<Self, ParseError<'i, ParserError<'i>>> {
    let location = input.current_source_location();
    let f = input.expect_function()?;
    let vendor_prefix = match_ignore_ascii_case! { &*f,
      "image-set" => VendorPrefix::None,
      "-webkit-image-set" => VendorPrefix::WebKit,
      _ => return Err(location.new_unexpected_token_error(
        cssparser::Token::Ident(f.clone())
      ))
    };

    let options = input.parse_nested_block(|input| {
      input.parse_comma_separated(ImageSetOption::parse)
    })?;
    Ok(ImageSet {
      options,
      vendor_prefix
    })
  }
}

impl<'i> ToCss for ImageSet<'i> {
  fn to_css<W>(&self, dest: &mut Printer<W>) -> Result<(), PrinterError> where W: std::fmt::Write {
    self.vendor_prefix.to_css(dest)?;
    dest.write_str("image-set(")?;
    let mut first = true;
    for option in &self.options {
      if first {
        first = false;
      } else {
        dest.delim(',', false)?;
      }
      option.to_css(dest, self.vendor_prefix != VendorPrefix::None)?;
    }
    dest.write_char(')')
  }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImageSetOption<'i> {
  pub image: Image<'i>,
  pub resolution: Resolution,
  pub file_type: Option<CowArcStr<'i>>
}

impl<'i> Parse<'i> for ImageSetOption<'i> {
  fn parse<'t>(input: &mut Parser<'i, 't>) -> Result<Self, ParseError<'i, ParserError<'i>>> {
    let loc = input.current_source_location();
    let image = if let Ok(url) = input.try_parse(|input| input.expect_url_or_string()) {
      Image::Url(Url {
        url: url.into(),
        loc
      })
    } else {
      Image::parse(input)?
    };

    let (resolution, file_type) = if let Ok(res) = input.try_parse(Resolution::parse) {
      let file_type = input.try_parse(parse_file_type).ok();
      (res, file_type)
    } else {
      let file_type = input.try_parse(parse_file_type).ok();
      let resolution = input.try_parse(Resolution::parse).unwrap_or(Resolution::Dppx(1.0));
      (resolution, file_type)
    };

    Ok(ImageSetOption { image, resolution, file_type: file_type.map(|x| x.into()) })
  }
}

impl<'i> ImageSetOption<'i> {
  fn to_css<W>(&self, dest: &mut Printer<W>, is_prefixed: bool) -> Result<(), PrinterError> where W: std::fmt::Write {
    match &self.image {
      // Prefixed syntax didn't allow strings, only url()
      Image::Url(url) if !is_prefixed => {
        // Add dependency if needed. Normally this is handled by the Url type.
        let dep = if dest.dependencies.is_some() {
          Some(UrlDependency::new(url, dest.filename()))
        } else {
          None
        };
        if let Some(dep) = dep {
          serialize_string(&dep.placeholder, dest)?;
          if let Some(dependencies) = &mut dest.dependencies {
            dependencies.push(Dependency::Url(dep))
          }
        } else {
          serialize_string(&url.url, dest)?;
        }
      },
      _ => self.image.to_css(dest)?
    }

    if self.resolution != Resolution::Dppx(1.0) {
      dest.write_char(' ')?;
      self.resolution.to_css(dest)?;
    }

    if let Some(file_type) = &self.file_type {
      dest.write_str(" type(")?;
      serialize_string(&file_type, dest)?;
      dest.write_char(')')?;
    }

    Ok(())
  }
}

fn parse_file_type<'i, 't>(input: &mut Parser<'i, 't>) -> Result<CowRcStr<'i>, ParseError<'i, ParserError<'i>>> {
  input.expect_function_matching("type")?;
  input.parse_nested_block(|input| Ok(input.expect_string_cloned()?))
}
