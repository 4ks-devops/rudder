use crate::error::*;
use crate::parser::*;
use super::context::VarContext;
use super::enums::{EnumList,EnumExpression};
use crate::ast::context::GlobalContext;

#[derive(Debug, PartialEq, Clone)]
pub struct StringObject<'src> {
    pos: Token<'src>,
    strs: Vec<String>,
    vars: Vec<String>,
}
impl<'src> StringObject<'src> {
    pub fn from_pstring(pos: Token<'src>, s: String) -> Result<StringObject> {
        let (strs, vars) = parse_string(&s[..])?;
        Ok(StringObject { pos, strs, vars })
    }
    pub fn format<SF, VF>(&self, str_formatter: SF, var_formatter: VF) -> String
    // string, is_a_suffix, is_a_prefix
    where
        SF: Fn(&str) -> String,
        VF: Fn(&str) -> String,
    {
        let mut output = String::new();
        let (last, elts) = self.strs.split_last().unwrap(); // strs cannot be empty
        let it = elts.iter().zip(self.vars.iter());
        for (s, v) in it {
            output.push_str(&str_formatter(s));
            output.push_str(&var_formatter(v));
        }
        output.push_str(&str_formatter(last));
        output
    }
    pub fn is_empty(&self) -> bool {
        self.vars.is_empty()
    }
}

#[derive(Debug, PartialEq, Clone)]
pub enum Value<'src> {
    //     position   format  variables
    String(StringObject<'src>),
    EnumExpression(EnumExpression<'src>),
}
impl<'src> Value<'src> {
    pub fn from_pvalue(pvalue: PValue<'src>/*, gc: &GlobalContext<'src>, lc: &Option<VarContext<'src>>*/) -> Result<Value<'src>> {
        match pvalue {
            PValue::String(pos, s) => Ok(Value::String(StringObject::from_pstring(pos, s)?)),
            PValue::EnumExpression(e) =>unimplemented!()
                /*Ok(Value::EnumExpression(
                gc.enum_list.canonify_expression(&Some(gc.var_context),lc,e)?
            )),*/
        }
    }

    pub fn from_static_pvalue(pvalue: PValue<'src>) -> Result<Value<'src>> {
        match pvalue {
            PValue::String(pos, s) => Ok(Value::String(StringObject::from_pstring(pos, s)?)),
            PValue::EnumExpression(e) => fail!(e.token(), "Enum expression are not allowed in static context"),
        }
    }

    // TODO check where it is called
    pub fn context_check(
        &self,
        gc: &GlobalContext<'src>,
        lc: Option<&VarContext<'src>>,
    ) -> Result<()> {
        match self {
            Value::String(s) => {
                fix_results(s.vars.iter().map(
                    |v| match gc.get_variable(lc, Token::new("", v)) {
                        None => fail!(s.pos, "Variable {} does not exist at {}", v, s.pos),
                        _ => Ok(()),
                    },
                ))
            },
            Value::EnumExpression(_) => Ok(()), // check already done at enum creation
        }
    }

    // TODO is it still useful given that it exists for PValue
    pub fn get_type(&self) -> PType {
        match self {
            Value::String(_) => PType::TString,
            Value::EnumExpression(_) => PType::TBoolean,
        }
    }
}
