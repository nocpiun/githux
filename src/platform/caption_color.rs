//! WinRT boxes for the Color values accepted by AppWindowTitleBar.
use windows::{
    Foundation::{
        DateTime, IPropertyValue, IPropertyValue_Impl, IReference, IReference_Impl, Point,
        PropertyType, Rect, Size, TimeSpan,
    },
    UI::Color,
    Win32::Foundation::TYPE_E_TYPEMISMATCH,
    core::{Array, Error, GUID, HSTRING, IInspectable, Result, implement},
};

#[implement(IReference<Color>, IPropertyValue)]
struct BoxedColor(Color);

impl IReference_Impl<Color> for BoxedColor_Impl {
    fn Value(&self) -> Result<Color> {
        Ok(self.0)
    }
}

macro_rules! unsupported_scalars {
    ($($method:ident: $ty:ty),* $(,)?) => {$(
        fn $method(&self) -> Result<$ty> {
            Err(Error::from_hresult(TYPE_E_TYPEMISMATCH))
        }
    )*};
}

macro_rules! unsupported_arrays {
    ($($method:ident: $ty:ty),* $(,)?) => {$(
        fn $method(&self, _value: &mut Array<$ty>) -> Result<()> {
            Err(Error::from_hresult(TYPE_E_TYPEMISMATCH))
        }
    )*};
}

impl IPropertyValue_Impl for BoxedColor_Impl {
    fn Type(&self) -> Result<PropertyType> {
        Ok(PropertyType::OtherType)
    }

    fn IsNumericScalar(&self) -> Result<bool> {
        Ok(false)
    }

    unsupported_scalars! {
        GetUInt8: u8, GetInt16: i16, GetUInt16: u16, GetInt32: i32, GetUInt32: u32,
        GetInt64: i64, GetUInt64: u64, GetSingle: f32, GetDouble: f64, GetChar16: u16,
        GetBoolean: bool, GetString: HSTRING, GetGuid: GUID, GetDateTime: DateTime,
        GetTimeSpan: TimeSpan, GetPoint: Point, GetSize: Size, GetRect: Rect,
    }
    unsupported_arrays! {
        GetUInt8Array: u8, GetInt16Array: i16, GetUInt16Array: u16, GetInt32Array: i32,
        GetUInt32Array: u32, GetInt64Array: i64, GetUInt64Array: u64, GetSingleArray: f32,
        GetDoubleArray: f64, GetChar16Array: u16, GetBooleanArray: bool,
        GetStringArray: HSTRING, GetInspectableArray: IInspectable, GetGuidArray: GUID,
        GetDateTimeArray: DateTime, GetTimeSpanArray: TimeSpan, GetPointArray: Point,
        GetSizeArray: Size, GetRectArray: Rect,
    }
}

pub fn boxed(color: gpui::Hsla) -> IReference<Color> {
    let color = color.to_rgb();
    BoxedColor(Color {
        A: (color.a * 255.0).round() as u8,
        R: (color.r * 255.0).round() as u8,
        G: (color.g * 255.0).round() as u8,
        B: (color.b * 255.0).round() as u8,
    })
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::core::Interface;

    #[test]
    fn boxed_color_preserves_channels_and_exposes_winrt_property_type() {
        let value = boxed(gpui::rgba(0x2468ac80).into());
        assert_eq!(
            value.Value().unwrap(),
            Color {
                A: 128,
                R: 36,
                G: 104,
                B: 172
            }
        );
        let property: IPropertyValue = value.cast().unwrap();
        assert_eq!(property.Type().unwrap(), PropertyType::OtherType);
        assert!(!property.IsNumericScalar().unwrap());
        assert_eq!(
            property.GetUInt32().unwrap_err().code(),
            TYPE_E_TYPEMISMATCH
        );
    }
}
