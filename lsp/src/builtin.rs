use zeen_ast::types::{BuiltinShape, ShapeLen};

pub(crate) fn is_scalar_type_name(name: &str) -> bool {
    matches!(
        name,
        "i8" | "i16"
            | "i32"
            | "i64"
            | "isize"
            | "u8"
            | "u16"
            | "u32"
            | "u64"
            | "usize"
            | "f32"
            | "f64"
            | "bool"
            | "char"
    )
}

pub(crate) fn strip_pointer_wrappers(ty: &str) -> &str {
    let mut name = ty.trim();

    loop {
        if name.starts_with('[') {
            return name;
        }

        if let Some(rest) = name.strip_prefix("*const ") {
            name = rest.trim_start();
            continue;
        }

        if let Some(rest) = name.strip_prefix('*') {
            name = rest.trim_start();
            continue;
        }

        return name;
    }
}

pub(crate) fn shape_matches_type(shape: &BuiltinShape, ty: &str) -> bool {
    let mut bindings = Vec::new();

    shape_matches_text(shape, strip_pointer_wrappers(ty), &mut bindings)
}

fn shape_matches_text(shape: &BuiltinShape, ty: &str, bindings: &mut Vec<(u32, String)>) -> bool {
    let ty = ty.trim();

    match shape {
        BuiltinShape::Scalar(builtin) => ty == builtin.to_string(),

        BuiltinShape::Slice { element, is_const } => {
            let Some(rest) = ty.strip_prefix("[]") else {
                return false;
            };

            let (found_const, rest) = strip_const(rest);

            found_const == *is_const && shape_matches_text(element, rest, bindings)
        }

        BuiltinShape::ManyPointer { element, is_const } => {
            let Some(rest) = ty.strip_prefix("[*]") else {
                return false;
            };

            let (found_const, rest) = strip_const(rest);

            found_const == *is_const && shape_matches_text(element, rest, bindings)
        }

        BuiltinShape::Array { element, len } => {
            let Some(rest) = ty.strip_prefix('[') else {
                return false;
            };

            let Some(close) = rest.find(']') else {
                return false;
            };

            let digits = rest[..close].trim();

            if digits.is_empty() {
                return false;
            }

            match len {
                ShapeLen::Known(expected) => {
                    if digits.parse::<u64>() != Ok(*expected) {
                        return false;
                    }
                }

                ShapeLen::Param(index) => match bindings.iter().find(|(slot, _)| slot == index) {
                    Some((_, bound)) if bound != digits => return false,
                    Some(_) => {}
                    None => bindings.push((*index, digits.to_string())),
                },
            }

            shape_matches_text(element, &rest[close + 1..], bindings)
        }

        BuiltinShape::Param(index) => match bindings.iter().find(|(slot, _)| slot == index) {
            Some((_, bound)) => bound == ty,
            None => {
                bindings.push((*index, ty.to_string()));

                true
            }
        },
    }
}

fn strip_const(ty: &str) -> (bool, &str) {
    match ty.trim_start().strip_prefix("const ") {
        Some(rest) => (true, rest.trim_start()),
        None => (false, ty.trim_start()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeen_ast::types::BuiltinType;

    fn array(len: ShapeLen) -> BuiltinShape {
        BuiltinShape::Array {
            element: Box::new(BuiltinShape::Scalar(BuiltinType::u8)),
            len,
        }
    }

    #[test]
    fn generic_array_shape_matches_concrete_text() {
        assert!(shape_matches_type(&array(ShapeLen::Param(0)), "[4]u8"));
        assert!(shape_matches_type(&array(ShapeLen::Param(0)), "[N]u8"));
        assert!(!shape_matches_type(&array(ShapeLen::Param(0)), "[]u8"));
    }

    #[test]
    fn concrete_array_shape_rejects_other_lengths() {
        assert!(shape_matches_type(&array(ShapeLen::Known(4)), "[4]u8"));
        assert!(!shape_matches_type(&array(ShapeLen::Known(4)), "[8]u8"));
    }
}
