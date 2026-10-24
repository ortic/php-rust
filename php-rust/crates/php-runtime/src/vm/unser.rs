//! `unserialize()` enum-case resolution (`E:` records, PHP 8.1). Split out so
//! the hostcall file does not grow: the pure parser is `crate::unserialize`,
//! the object builder is `Vm::vm_ser_build`.

use super::*;

impl<'m> super::Vm<'m> {
    /// Zend resolves each `E:` record while parsing (`var_unserializer.re`):
    /// a name without `:`, an unknown class (autoloaded first) or a non-enum
    /// class fail at the token's START; an unknown name, or a constant that is
    /// not a case, fail at its END — each after its own warning. Returns the
    /// offset of the first failure in serialization order, `None` when every
    /// enum resolves.
    pub(super) fn unser_check_enums(&mut self, s: &crate::unserialize::Ser) -> Result<Option<usize>, PhpError> {
        use crate::unserialize::Ser;
        let children: Vec<&Ser> = match s {
            Ser::Array(items) => items.iter().map(|(_, v)| v).collect(),
            Ser::Object(_, props) => props.iter().map(|(_, v)| v).collect(),
            Ser::Enum(name, start, end) => {
                let lossy = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
                let Some(colon) = name.iter().position(|&c| c == b':') else {
                    self.diags.push(Diag::Warning(format!(
                        "unserialize(): Invalid enum name '{}' (missing colon)",
                        lossy(name)
                    )));
                    return Ok(Some(*start));
                };
                let (cls, case) = (&name[..colon], &name[colon + 1..]);
                // zend_is_valid_class_name: an invalid name fails silently.
                if !cls.iter().all(|&c| c.is_ascii_alphanumeric() || c == b'_' || c == b'\\' || c >= 0x80) {
                    return Ok(Some(*start));
                }
                let Some(cid) = self.resolve_class_autoload(cls)? else {
                    self.diags.push(Diag::Warning(format!("unserialize(): Class '{}' not found", lossy(cls))));
                    return Ok(Some(*start));
                };
                if !matches!(self.classes[cid].instantiable, crate::bytecode::Instantiable::Enum) {
                    self.diags.push(Diag::Warning(format!("unserialize(): Class '{}' is not an enum", lossy(cls))));
                    return Ok(Some(*start));
                }
                if self.enum_case_idx(cid, case).is_none() {
                    self.diags.push(Diag::Warning(if find_const_runtime(&self.classes, cid, case).is_some() {
                        format!("unserialize(): {}::{} is not an enum case", lossy(cls), lossy(case))
                    } else {
                        format!("unserialize(): Undefined constant {}::{}", lossy(cls), lossy(case))
                    }));
                    return Ok(Some(*end));
                }
                return Ok(None);
            }
            _ => return Ok(None),
        };
        for c in children {
            if let Some(off) = self.unser_check_enums(c)? {
                return Ok(Some(off));
            }
        }
        Ok(None)
    }

    /// The case object an `E:` record names — already validated by
    /// [`Self::unser_check_enums`], so a miss can only be a stale tree (`None`).
    pub(super) fn unser_enum_case(&mut self, name: &[u8]) -> Option<Zval> {
        let colon = name.iter().position(|&c| c == b':')?;
        let cls = name[..colon].strip_prefix(b"\\").unwrap_or(&name[..colon]);
        let cid = *self.class_index.get(LcKey::new(cls).as_slice())?;
        let idx = self.enum_case_idx(cid, &name[colon + 1..])?;
        Some(Zval::Object(self.enum_case(cid, idx as u32)))
    }
}
