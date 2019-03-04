use super::context::{VarContext, VarKind, GlobalContext};
use super::resource::Statement;
use crate::error::*;
use crate::parser::{PEnum, PEnumExpression, PEnumMapping, Token};
use std::collections::hash_map;
use std::collections::hash_set::Iter;
use std::collections::HashMap;
use std::collections::HashSet;

/// As single enum can be derived into different set through multiple mappings
/// However a single enum can have only one parent (single ascending path, multiple descending paths)

/// Structure that contains all enums
/// It can get complex because it contains hashmap for all possible way we may way to query them
#[derive(Debug)]
pub struct EnumList<'src> {
    // Map an enum name to another one which has a derived definition
    //                            to         from
    reverse_mapping_path: HashMap<Token<'src>, Token<'src>>,
    //                           from       to
    direct_mapping_path: HashMap<Token<'src>, Vec<Token<'src>>>,
    // Map an enum content to another one
    //         enum    from       to          mapping from       to
    mappings: HashMap<(Token<'src>, Token<'src>), HashMap<Token<'src>, Token<'src>>>,
    // List values for a given enum
    //             enum       global values
    enums: HashMap<Token<'src>, (bool, HashSet<Token<'src>>)>,
    // List global values (they must not be redefined)
    //                    value      enum
    pub global_values: HashMap<Token<'src>, Token<'src>>,
}

/// A boolean expression that can be defined using enums
#[derive(Debug, PartialEq, Clone)]
pub enum EnumExpression<'src> {
    //      variable   enum        value
    Compare(Token<'src>, Token<'src>, Token<'src>),
    And(Box<EnumExpression<'src>>, Box<EnumExpression<'src>>),
    Or(Box<EnumExpression<'src>>, Box<EnumExpression<'src>>),
    Not(Box<EnumExpression<'src>>),
    Default(Token<'src>), // token for position handling only
}

impl<'src> EnumExpression<'src> {
    /// print the expression position in source code for user output
    pub fn position_str(&self) -> String {
        let (file, line, col) = self.position();
        format!("{}:{}:{}", file, line, col)
    }
    /// extract the position triplet
    pub fn position(&self) -> (String, u32, usize) {
        match self {
            EnumExpression::Compare(_, _, v) => v.position(),
            EnumExpression::And(a, _) => a.position(),
            EnumExpression::Or(a, _) => a.position(),
            EnumExpression::Not(a) => a.position(),
            EnumExpression::Default(a) => a.position(),
        }
    }
    /// return true if this is just the expression 'default'
    pub fn is_default(&self) -> bool {
        match self {
            EnumExpression::Default(_) => true,
            _ => false,
        }
    }
}

impl<'src> EnumList<'src> {
    /// Default constructor
    pub fn new() -> EnumList<'static> {
        EnumList {
            reverse_mapping_path: HashMap::new(),
            direct_mapping_path: HashMap::new(),
            mappings: HashMap::new(),
            enums: HashMap::new(),
            global_values: HashMap::new(),
        }
    }

    pub fn iter(&self) -> hash_map::Iter<Token<'src>, (bool, HashSet<Token<'src>>)> {
        self.enums.iter()
    }
    /// Returns true if a given enum exists
    pub fn enum_exists(&self, e: Token<'src>) -> bool {
        self.enums.contains_key(&e)
    }

    /// Returns true if the given enum is global
    /// Be careful: it panics if the enum doesn't exist!
    pub fn is_global(&self, e: Token<'src>) -> bool {
        self.enums[&e].0
    }

    /// Insert a simple enum content (not a mapping) into the global structure
    pub fn add_enum(&mut self, e: PEnum<'src>) -> Result<()> {
        let mut list = HashSet::new();
        // Check for set name duplicate
        if self.enums.contains_key(&e.name) {
            // trick to extract the original key, since they do not have the same debug info
            let position = self.enums.entry(e.name).key().position_str();
            fail!(
                self.enums.entry(e.name).key(),
                "Enum {} already defined at {}",
                e.name,
                position
            );
        }
        // check that enum is not empty
        if e.items.is_empty() {
            fail!(
                e.name,
                "Enums must have at least one item, {} is empty",
                e.name
            );
        }
        let parent_enum = self.reverse_mapping_path.get(&e.name);
        for v in e.items {
            // Check for local uniqueness (not for mapping)
            if list.contains(&v) && parent_enum.is_none() {
                fail!(
                    v,
                    "Value {} already declared in the same enum {}",
                    v,
                    e.name
                );
            }
            // check for global uniqueness
            // defined in parent is allowed, twice in the same mapping is allowed
            if let Some(e0) = self.global_values.get(&v) {
                if parent_enum.is_none() || (parent_enum.unwrap() != e0 && e.name != *e0) {
                    fail!(
                        v,
                        "Value {} from enum {} already declared in the global enum {} and not in parent enum",
                        v,
                        e.name,
                        e0
                    );
                }
            }
            // store globally uniques
            if e.global {
                // If the value already exist, update its enum
                // WARN: but do not update its position
                self.global_values.insert(v, e.name);
            }
            // keep value
            list.insert(v);
        }
        // store data
        self.enums.insert(e.name, (e.global, list));
        Ok(())
    }

    /// Insert an enum defined from a mapping into the global structure
    #[allow(clippy::map_entry)]
    pub fn add_mapping(&mut self, e: PEnumMapping<'src>) -> Result<()> {
        // From must exist
        match self.enums.get(&e.from) {
            Some((global, values)) => {
                // transform mapping into temporary hashmap
                let mut pmapping = HashMap::new();
                for (f, t) in e.mapping {
                    // check for duplicates
                    if pmapping.contains_key(&f) {
                        fail!(f, "{} used twice in mapping {}", f, e.to);
                    }
                    // Check for invalids
                    if !(values.contains(&f) || *f == "*") {
                        fail!(
                            f,
                            "Value {} used in mapping {} does not exist in {}",
                            f,
                            e.to,
                            *e.from
                        );
                    }
                    pmapping.insert(f, t);
                }
                // store mapping into final hashmap
                let mut mapping = HashMap::new();
                let mut items: Vec<Token> = Vec::new();
                for v in values {
                    match pmapping.get(v).or_else(|| pmapping.get(&"*".into())) {
                        Some(t) => {
                            if **t == "*" {
                                mapping.insert(*v, *v);
                                items.push(*v);
                            } else {
                                mapping.insert(*v, *t);
                                items.push(*t);
                            }
                        }
                        None => fail!(
                            v,
                            "Value {} defined in {} is not used in mapping {}",
                            v,
                            *e.from,
                            e.to
                        ),
                    }
                }
                self.reverse_mapping_path.insert(e.to, e.from);
                if self.direct_mapping_path.contains_key(&e.from) {
                    self.direct_mapping_path
                        .get_mut(&e.from)
                        .unwrap()
                        .push(e.to);
                } else {
                    self.direct_mapping_path.insert(e.from, vec![e.to]);
                }
                self.mappings.insert((e.from, e.to), mapping);
                self.add_enum(PEnum {
                    global: *global,
                    name: e.to,
                    items,
                })
            }
            None => fail!(
                e.to,
                "Enum {} missing when trying to define {}",
                e.from,
                e.to
            ),
            //None => err( format!("Enum {} missing when trying to define {}", e.from, e.to) ),
        }
    }

    /// Return an iterator over an enum content
    /// Be careful: it panics if the enum doesn't exist!
    pub fn enum_iter<'b>(&'b self, e: Token<'src>) -> Iter<'b, Token<'src>> {
        self.enums[&e].1.iter()
    }

    /// Find ascending enum path from enum e1 to enum e2
    fn find_path<'b>(&'b self, e1: Token<'src>, e2: Token<'src>) -> Option<Vec<Token<'src>>> {
        // terminate recursion
        if e1 == e2 {
            Some(vec![e1])
        } else {
            // traverse mapping path starting from the end
            match self.reverse_mapping_path.get(&e2) {
                None => None,
                Some(e) => match self.find_path(e1, *e) {
                    None => None,
                    Some(mut path) => {
                        path.push(e2);
                        Some(path)
                    }
                },
            }
        }
    }

    /// Find oldest ancestor of enum e1 following ascending path
    fn find_elder(&self, e1: Token<'src>) -> Token<'src> {
        match self.reverse_mapping_path.get(&e1) {
            None => e1,
            Some(e) => self.find_elder(*e),
        }
    }

    /// Find the last descendant enum of e1 that has the same name as item
    pub fn find_descendant_enum(&self, e1: Token<'src>, item: Token<'src>) -> Token<'src> {
        match self.direct_mapping_path.get(&e1) {
            None => return e1,
            Some(e2) => {
                for to in e2 {
                    let new_item = self.mappings[&(e1, *to)][&item];
                    if new_item == item {
                        return self.find_descendant_enum(*to, item);
                    }
                }
            }
        }
        e1
    }

    /// Transforms a parsed enum expression into its final form using all enum definitions
    /// It needs a variable context (local and global) to check for proper variable existence
    pub fn canonify_expression<'b>(
        &'b self,
        gc: &'b GlobalContext<'src>,
        lc: Option<&'b VarContext<'src>>,
        expr: PEnumExpression<'src>,
    ) -> Result<EnumExpression<'src>> {
        match expr {
            PEnumExpression::Default(t) => Ok(EnumExpression::Default(t)),
            PEnumExpression::Not(e) => Ok(EnumExpression::Not(Box::new(
                self.canonify_expression(gc, lc, *e)?,
            ))),
            PEnumExpression::Or(e1, e2) => Ok(EnumExpression::Or(
                Box::new(self.canonify_expression(gc, lc, *e1)?),
                Box::new(self.canonify_expression(gc, lc, *e2)?),
            )),
            PEnumExpression::And(e1, e2) => Ok(EnumExpression::And(
                Box::new(self.canonify_expression(gc, lc, *e1)?),
                Box::new(self.canonify_expression(gc, lc, *e2)?),
            )),
            PEnumExpression::Compare(var, enum1, value) => {
                // get enum1 real type
                let e1 = match enum1 {
                    Some(e) => e,
                    None => match self.global_values.get(&value) {
                        // global enum ?
                        Some(e) => *e,
                        // none -> try to guess from var
                        None => match var {
                            // None -> this may be a boolean
                            // when using a boolean, value is a variable name since the parser doesn't know how to tell the difference between both syntax
                            None => match gc.get_variable(lc, value) {
                                Some(VarKind::Enum(t, _)) => {
                                    if *t == Token::new("", "boolean") {
                                        *t
                                    } else {
                                        fail!(value, "Variable {} must be compared with some enum in expression", value)
                                    }
                                }
                                _ => fail!(value, "Global enum value {} does not exist", value),
                            },
                            Some(var1) => match gc.get_variable(lc, var1) {
                                Some(VarKind::Enum(t, _)) => *t,
                                _ => fail!(
                                    var1,
                                    "Variable {} doesn't exist or doesn't have an enum type",
                                    var1
                                ),
                            },
                        },
                    },
                };
                // get var real name
                let var1 = match var {
                    Some(v) => v,
                    None => {
                        if e1 == Token::new("", "boolean") {
                            // special handling of booleans since they are messed up by the parser
                            value
                        } else {
                            match self.enums.get(&e1) {
                                // or get it from a global enum
                                None => fail!(e1, "No such enum {}", e1),
                                Some((false, _)) => fail!(
                                    e1,
                                    "Enum {} is not global, you must provide a variable",
                                    e1
                                ),
                                Some((true, _)) => self.find_elder(e1),
                            }
                        }
                    }
                };
                let val = if e1 == Token::new("", "boolean") {
                    Token::new("internal", "true")
                } else {
                    value
                };
                // check that enum exists and has value
                match self.enums.get(&e1) {
                    None => fail!(e1, "Enum {} does not exist", e1),
                    Some((_, list)) => {
                        if !list.contains(&val) {
                            fail!(val, "Value {} is not defined in enum {}", val, e1)
                        }
                    }
                }
                // check that var exists
                match gc.get_variable(lc, var1) {
                    None => fail!(var1, "Variable {} does not exist", var1),
                    // wrong enum type
                    Some(VarKind::Enum(t, _)) => {
                        // check variable enum type
                        if self.find_path(*t, e1).is_none() {
                            fail!(
                                var1,
                                "Variable {} is {} but expected to be {} or an ancestor",
                                var1,
                                t,
                                e1
                            );
                        }
                    }
                    // not an enum
                    _ => fail!(var1, "Variable {} is not a {} enum", var1, e1),
                }
                Ok(EnumExpression::Compare(var1, e1, val))
            }
        }
    }

    /// Transforms a value name into its ancestor following an ancestor path
    /// only used by is_ancestor (path is destroyed)
    fn transform_value(&self, mut path: Vec<Token<'src>>, value: Token<'src>) -> Token<'src> {
        // <1 should not happen
        if path.len() <= 1 {
            return value;
        }
        let e0 = path.remove(0);
        let e1 = path[0];
        let v = self.mappings[&(e0, e1)][&value];
        self.transform_value(path, v)
    }
    /// Returns true is e1:v1 is an ancestor of e2:v2
    pub fn is_ancestor(
        &self,
        e1: Token<'src>,
        v1: Token<'src>,
        e2: Token<'src>,
        v2: Token<'src>,
    ) -> bool {
        match self.find_path(e1, e2) {
            Some(path) => self.transform_value(path, v1) == v2,
            None => false,
        }
    }

    /// Evaluate a boolean expression (final version) given a set of variable=enum:value
    /// We need the variables to have a real value to evaluate to a boolean
    fn eval(
        &self,
        values: &HashMap<Token<'src>, (Token<'src>, Token<'src>)>,
        expr: &EnumExpression,
    ) -> bool {
        match expr {
            EnumExpression::Default(_) => true,
            EnumExpression::Not(e) => !self.eval(values, &e),
            EnumExpression::Or(e1, e2) => self.eval(values, &e1) || self.eval(values, &e2),
            EnumExpression::And(e1, e2) => self.eval(values, &e1) && self.eval(values, &e2),
            EnumExpression::Compare(var, enum1, value) => {
                // unwrap panic would be a bug since all values should be defined now
                let (s, v) = values.get(&var).unwrap();
                self.is_ancestor(*s, *v, *enum1, *value)
            }
        }
    }

    /// List all variables that are used in an expression,
    /// and put them into the 'variables' hashset
    /// this is recursive mutable, pass it an empty hashset at first call
    /// Only used by evaluate.
    fn list_variable_enum(
        &self,
        variables: &mut HashSet<Token<'src>>,
        expr: &EnumExpression<'src>,
    ) {
        match expr {
            EnumExpression::Default(_) => (),
            EnumExpression::Not(e) => self.list_variable_enum(variables, e),
            EnumExpression::Or(e1, e2) => {
                self.list_variable_enum(variables, e1);
                self.list_variable_enum(variables, e2);
            }
            EnumExpression::And(e1, e2) => {
                self.list_variable_enum(variables, e1);
                self.list_variable_enum(variables, e2);
            }
            EnumExpression::Compare(var, _, _) => {
                variables.insert(*var);
            }
        }
    }

    /// Describe a set a variable values as a valid expression for the user
    fn describe(&self, values: &HashMap<Token<'src>, (Token<'src>, Token<'src>)>) -> String {
        values
            .iter()
            .map(|(key, (e, val))| {
                format!("{}=~{}:{}", key.fragment(), e.fragment(), val.fragment())
            })
            .collect::<Vec<String>>()
            .join(" && ")
    }

    /// Evaluates a set of expressions possible outcome to check for missing cases and redundancy
    /// Variable context is here to guess variable type and to properly evaluate variables that are constant
    pub fn evaluate<'b>(
        &self,
        gc: &'b GlobalContext<'src>,
        lc: Option<&'b VarContext<'src>>,
        cases: &[(EnumExpression<'src>, Vec<Statement<'src>>)],
        case_name: Token<'src>,
    ) -> Result<()> {
        let mut variables = HashSet::new();
        cases
            .iter()
            .for_each(|(e, _)| self.list_variable_enum(&mut variables, &e));
        let it = ContextIterator::new(gc, lc, variables.into_iter().collect());
        let mut default_used = false;
        fix_results(it.map(|values| {
            let mut matched_exp = cases.iter().filter(|(e,_)| self.eval(&values, e));
            match matched_exp.next() {
                // Missing case
                None => fail!(case_name, "Missing case in {}, '{}' is never processed", case_name, self.describe(&values)),
                Some((e1,_)) => match matched_exp.next() {
                    // Single matching case
                    None => Ok(()),
                    // Overlapping cases
                    Some((e2,_)) => if !e1.is_default() && !e2.is_default() {
                        fail!(case_name,"Duplicate case at {} and {}, '{}' is processed twice, result may be unexpected",e1.position_str(),e2.position_str(),self.describe(&values))
                    } else {
                        default_used = true;
                        Ok(())
                    },
                },
            }
        }))?;
        if !default_used && cases.iter().any(|(e, _)| e.is_default()) {
            fail!(case_name, "Default never matches in {}", case_name);
        }
        Ok(())
    }

    /// There can be only one mapping per item that defines an identical descendant
    pub fn mapping_check(&self) -> Result<()> {
        for (from, tos) in self.direct_mapping_path.iter() {
            for item in self.enums[from].1.iter() {
                if tos
                    .iter()
                    .map(|to| self.mappings[&(*from, *to)][item])
                    .filter(|i| i == item)
                    .count()
                    > 1
                {
                    fail!(
                        item,
                        "There is more than one mapping that maps {}:{} to itself",
                        from,
                        item
                    );
                }
            }
        }
        Ok(())
    }
}

/// Type to store either a constant value or an iterable value
/// only used by ContextIterator
// lifetime should be 'src,'b but as long as it works it's easier like this
enum AltVal<'src> {
    Constant(Token<'src>),
    Iterator(Iter<'src, Token<'src>>),
}
/// Iterator that can iterate over all possible value set that a variable set can take
/// Ex: if var1 and var2 are variables of an enum type with 3 item
///     it will produce 9 possible value combinations
// lifetime should be 'src,'b but as long as it works it's easier like this
struct ContextIterator<'src> {
    // enum reference
    enum_list: &'src EnumList<'src>,
    // variables to vary
    var_list: Vec<Token<'src>>,
    //                 name        value or iterator
    iterators: HashMap<Token<'src>, AltVal<'src>>,
    // current iteration         enum       value
    current: HashMap<Token<'src>, (Token<'src>, Token<'src>)>,
    // true before first iteration
    first: bool,
}

impl<'src> ContextIterator<'src> {
    /// Create the iterator from the global enum_list
    /// a variable context (for type and constants) and the list of variables to take
    /// (since the context may contain variables that are not used in the expression)
    fn new(
        gc: &'src GlobalContext<'src>,
        lc: Option<&VarContext<'src>>,
        var_list: Vec<Token<'src>>,
    ) -> ContextIterator<'src> {
        let mut iterators = HashMap::new();
        let mut current = HashMap::new();
        for v in &var_list {
            match gc.get_variable(lc, *v) {
                // known value
                Some(VarKind::Enum(e, Some(val))) => {
                    current.insert(*v, (*e, *val));
                    iterators.insert(*v, AltVal::Constant(*val));
                }
                // iterable value
                Some(VarKind::Enum(e, None)) => {
                    let mut it = gc.enum_list.enum_iter(*e);
                    current.insert(*v, (*e, *(it.next().unwrap()))); // enums must always have at least one value
                    iterators.insert(*v, AltVal::Iterator(it));
                }
                // impossible values
                None => panic!("BUG This missing var should have already been detected"),
                Some(_) => panic!("BUG This mistyped var should have already been detected"),
            };
        }
        ContextIterator {
            enum_list: &gc.enum_list,
            var_list,
            iterators,
            current,
            first: true,
        }
    }
}

impl<'src> Iterator for ContextIterator<'src> {
    type Item = HashMap<Token<'src>, (Token<'src>, Token<'src>)>;
    /// Iterate: we store an iterator for each variables
    /// - we increment the first iterator
    /// - if it ended reset it and increment the next iterator
    /// - continue until we reach the end of the last iterator
    fn next(&mut self) -> Option<Self::Item> {
        if self.first {
            self.first = false;
            // We cannot return a reference into self, so we must clone
            // probably not the best solution but at least it's correct
            return Some(self.current.clone());
        }
        for v in &self.var_list {
            let reset = match self.iterators.get_mut(v) {
                Some(AltVal::Constant(_)) => false, // skip this var
                Some(AltVal::Iterator(i)) => match i.next() {
                    None => true, // restart this iterator with current enum
                    Some(val) => {
                        // get next value in iterator and return
                        self.current.entry(*v).and_modify(|e| e.1 = *val);
                        return Some(self.current.clone());
                    }
                },
                _ => panic!("BUG some value disappeared"),
            };
            if reset {
                if let Some((e, _)) = self.current.get(v) {
                    // restart iterator and update current
                    let mut it = self.enum_list.enum_iter(*e);
                    self.current.insert(*v, (*e, *(it.next().unwrap())));
                    self.iterators.insert(*v, AltVal::Iterator(it));
                } // no else, it has been checked many times
            }
        }
        // no change or all iterator restarted -> the end
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::*;
    use maplit::hashmap;
    use std::iter::FromIterator;

    // test utilities
    fn add_penum<'src>(e: &mut EnumList<'src>, string: &'src str) -> Result<()> {
        e.add_enum(penum(pinput("", string)).unwrap().1)
    }

    fn add_enum_mapping<'src>(e: &mut EnumList<'src>, string: &'src str) -> Result<()> {
        e.add_mapping(penum_mapping(pinput("", string)).unwrap().1)
    }

    fn parse_enum_expression(string: &str) -> PEnumExpression {
        penum_expression(pinput("", string)).unwrap().1
    }

    fn ident(string: &str) -> Token {
        pidentifier(pinput("", string)).unwrap().1
    }

    #[test]
    fn test_insert() {
        let ref mut e = EnumList::new();
        assert!(add_penum(e, "enum abc { a, a, c }").is_err());
        assert!(add_penum(e, "global enum abc { a, b, c }").is_ok());
        assert!(add_penum(e, "enum abc { a, b, c }").is_err());
        assert!(add_penum(e, "enum abc2 { a, b, c }").is_err());
        assert!(add_enum_mapping(e, "enum abc ~> def { a -> b, b -> b }").is_err());
        assert!(add_enum_mapping(e, "enum abx ~> def { a -> b, b -> b, c->c }").is_err());
        assert!(add_enum_mapping(e, "enum abc ~> abc { a -> b, b -> b, c->c }").is_err());
        assert!(add_enum_mapping(e, "enum abc ~> def { a -> b, b -> b, x->c }").is_err());
        assert!(add_enum_mapping(e, "enum abc ~> def { a -> b, b -> b, *->* }").is_ok());
    }

    fn init_tests() -> GlobalContext<'static> {
        let mut e = EnumList::new();
        add_penum(
            &mut e,
            "global enum os { debian, ubuntu, redhat, centos, aix }",
        )
        .unwrap();
        add_enum_mapping(
            &mut e,
            "enum os ~> family { ubuntu->debian, centos->redhat, *->* }",
        )
        .unwrap();
        add_enum_mapping(
            &mut e,
            "enum family ~> type { debian->linux, redhat->linux, aix->unix }",
        )
        .unwrap();
        add_penum(&mut e, "enum boolean { true, false }").unwrap();
        add_penum(&mut e, "enum outcome { kept, repaired, error }").unwrap();
        add_enum_mapping(
            &mut e,
            "enum outcome ~> okerr { kept->ok, repaired->ok, error->error }",
        )
        .unwrap();
        let mut context = VarContext::new();
        context.new_enum_variable(None, ident("os"), ident("os"), None)
            .unwrap();
        context.new_enum_variable(None, ident("abool"), ident("boolean"), None)
            .unwrap();
        context.new_enum_variable(None, ident("out"), ident("outcome"), None)
            .unwrap();
        context.new_enum_variable(None, ident("in"), ident("outcome"), Some(ident("kept")))
            .unwrap();
        GlobalContext {
            enum_list: e,
            var_context: context,
            parameter_defaults: HashMap::new(),
        }
    }

    #[test]
    fn test_path() {
        let gc = init_tests();
        assert_eq!(gc.enum_list.find_path("os".into(), "outcome".into()), None);
        assert_eq!(
            gc.enum_list.find_path("os".into(), "os".into()),
            Some(vec!["os".into()])
        );
        assert_eq!(
            gc.enum_list.find_path("os".into(), "type".into()),
            Some(vec!["os".into(), "family".into(), "type".into()])
        );
        assert_eq!(gc.enum_list.find_path("type".into(), "os".into()), None);
    }

    #[test]
    fn test_ancestor() {
        let gc = init_tests();
        assert_eq!(
            gc.enum_list.is_ancestor("os".into(), "ubuntu".into(), "os".into(), "ubuntu".into()),
            true
        );
        assert_eq!(
            gc.enum_list.is_ancestor("os".into(), "ubuntu".into(), "os".into(), "debian".into()),
            false
        );
        assert_eq!(
            gc.enum_list.is_ancestor(
                "os".into(),
                "ubuntu".into(),
                "family".into(),
                "debian".into()
            ),
            true
        );
        assert_eq!(
            gc.enum_list.is_ancestor("os".into(), "ubuntu".into(), "type".into(), "linux".into()),
            true
        );
        assert_eq!(
            gc.enum_list.is_ancestor("os".into(), "ubuntu".into(), "type".into(), "unix".into()),
            false
        );
        assert_eq!(
            gc.enum_list.is_ancestor(
                "os".into(),
                "ubuntu".into(),
                "outcome".into(),
                "kept".into()
            ),
            false
        );
        assert_eq!(
            gc.enum_list.is_ancestor(
                "os".into(),
                "ubuntu".into(),
                "outcome".into(),
                "debian".into()
            ),
            false
        );
    }

    #[test]
    fn test_elder() {
        let gc = init_tests();
        assert_eq!(gc.enum_list.find_elder("os".into()), "os".into());
        assert_eq!(gc.enum_list.find_elder("type".into()), "os".into());
        assert_eq!(gc.enum_list.find_elder("outcome".into()), "outcome".into());
    }

    #[test]
    fn test_descendant() {
        let gc = init_tests();
        assert_eq!(
            gc.enum_list.find_descendant_enum("os".into(), "debian".into()),
            "family".into()
        );
        assert_eq!(
            gc.enum_list.find_descendant_enum("os".into(), "ubuntu".into()),
            "os".into()
        );
        assert_eq!(
            gc.enum_list.find_descendant_enum("outcome".into(), "kept".into()),
            "outcome".into()
        );
    }

    #[test]
    fn test_canonify() {
        let gc = init_tests();
        assert!(gc.enum_list
            .canonify_expression(&gc, None, parse_enum_expression("abool"))
            .is_ok());
        assert!(gc.enum_list
            .canonify_expression(&gc, None, parse_enum_expression("ubuntu"))
            .is_ok());
        assert!(gc.enum_list
            .canonify_expression(&gc, None, parse_enum_expression("os:ubuntu"))
            .is_ok());
        assert!(gc.enum_list
            .canonify_expression(&gc, None, parse_enum_expression("os=~ubuntu"))
            .is_ok());
        assert!(gc.enum_list
            .canonify_expression(&gc, None, parse_enum_expression("os!~ubuntu"))
            .is_ok());
        assert!(gc.enum_list
            .canonify_expression(&gc, None, parse_enum_expression("os=~os:ubuntu"))
            .is_ok());
        assert!(gc.enum_list
            .canonify_expression(&gc, None, parse_enum_expression("os=~linux"))
            .is_ok());
        assert!(gc.enum_list
            .canonify_expression(&gc, None, parse_enum_expression("out=~linux"))
            .is_err());
        assert!(gc.enum_list
            .canonify_expression(&gc, None, parse_enum_expression("os=~outcome:kept"))
            .is_err());
        assert!(gc.enum_list
            .canonify_expression(&gc, None, parse_enum_expression("os=~type:linux"))
            .is_ok());
        assert!(gc.enum_list
            .canonify_expression(&gc, None, parse_enum_expression("os=~type:debian"))
            .is_err());
        assert!(gc.enum_list
            .canonify_expression(&gc, None, parse_enum_expression("os=~typo:debian"))
            .is_err());
        assert!(gc.enum_list
            .canonify_expression(&gc, None, parse_enum_expression("typo:debian"))
            .is_err());
        assert!(gc.enum_list
            .canonify_expression(&gc, None, parse_enum_expression("outcome:kept"))
            .is_err());
        assert!(gc.enum_list
            .canonify_expression(&gc, None, parse_enum_expression("kept"))
            .is_err());
    }

    #[test]
    fn test_eval() {
        let gc = init_tests();
        assert!(gc.enum_list.eval(
            &hashmap! { Token::from("abool") => (Token::from("boolean"),Token::from("true")) },
            &gc.enum_list.canonify_expression(&gc, None, parse_enum_expression("abool"))
                .unwrap()
        ));
        assert!(gc.enum_list.eval(
            &hashmap! { Token::from("os") => (Token::from("os"),Token::from("ubuntu")) },
            &gc.enum_list.canonify_expression(&gc, None, parse_enum_expression("ubuntu"))
                .unwrap()
        ));
        assert!(gc.enum_list.eval(
            &hashmap! { Token::from("os") => (Token::from("os"),Token::from("ubuntu")) },
            &gc.enum_list.canonify_expression(&gc, None, parse_enum_expression("debian"))
                .unwrap()
        ));
        assert!(!gc.enum_list.eval(
            &hashmap! { Token::from("os") => (Token::from("os"),Token::from("ubuntu")) },
            &gc.enum_list.canonify_expression(&gc, None, parse_enum_expression("os:debian"))
                .unwrap()
        ));
        assert!(gc.enum_list.eval(
            &hashmap! { Token::from("os") => (Token::from("os"),Token::from("ubuntu")) },
            &gc.enum_list.canonify_expression(&gc, None, parse_enum_expression("!os:debian"))
                .unwrap()
        ));
        assert!(!gc.enum_list.eval(&hashmap! { Token::from("os") => (Token::from("os"),Token::from("ubuntu")), Token::from("out") => (Token::from("outcome"),Token::from("kept")) },
                        &gc.enum_list.canonify_expression(&gc, None, parse_enum_expression("os:debian && out =~ outcome:kept")).unwrap()));
        assert!(gc.enum_list.eval(&hashmap! { Token::from("os") => (Token::from("os"),Token::from("ubuntu")), Token::from("out") => (Token::from("outcome"),Token::from("kept")) },
                       &gc.enum_list.canonify_expression(&gc, None, parse_enum_expression("os:debian || out =~ outcome:kept")).unwrap()));
    }

    #[test]
    fn test_varlist() {
        let gc = init_tests();
        {
            let mut var1 = HashSet::new();
            let ex = parse_enum_expression("os:debian");
            let exp = gc.enum_list.canonify_expression(&gc, None, ex).unwrap();
            gc.enum_list.list_variable_enum(&mut var1, &exp);
            assert_eq!(
                var1,
                HashSet::from_iter(vec![Token::from("os")].into_iter())
            );
        }
        {
            let mut var1 = HashSet::new();
            let ex = parse_enum_expression("os:debian && out =~ outcome:kept");
            let exp = gc.enum_list.canonify_expression(&gc, None, ex).unwrap();
            gc.enum_list.list_variable_enum(&mut var1, &exp);
            assert_eq!(
                var1,
                HashSet::from_iter(vec![Token::from("os"), Token::from("out")])
            );
        }
        {
            let mut var1 = HashSet::new();
            let ex = parse_enum_expression("family:debian && os:ubuntu");
            let exp = gc.enum_list.canonify_expression(&gc, None, ex).unwrap();
            gc.enum_list.list_variable_enum(&mut var1, &exp);
            assert_eq!(var1, HashSet::from_iter(vec![Token::from("os")]));
        }
        {
            let mut var1 = HashSet::new();
            let ex1 = parse_enum_expression("family:debian && os:ubuntu");
            let exp1 = gc.enum_list.canonify_expression(&gc, None, ex1).unwrap();
            gc.enum_list.list_variable_enum(&mut var1, &exp1);
            let ex2 = parse_enum_expression("os:debian && out =~ outcome:kept");
            let exp2 = gc.enum_list.canonify_expression(&gc, None, ex2).unwrap();
            gc.enum_list.list_variable_enum(&mut var1, &exp2);
            assert_eq!(
                var1,
                HashSet::from_iter(vec![Token::from("os"), Token::from("out")])
            );
        }
    }

    #[test]
    fn test_iterator() {
        let gc = init_tests();
        let mut varlist = HashSet::new();
        let ex = parse_enum_expression("os:debian && out =~ outcome:kept");
        let exp = gc.enum_list.canonify_expression(&gc, None, ex).unwrap();
        gc.enum_list.list_variable_enum(&mut varlist, &exp);
        let it = ContextIterator::new(&gc, None, varlist.into_iter().collect());
        assert_eq!(it.count(), 15);
    }

    #[test]
    fn test_evaluate() {
        let gc = init_tests();
        let case = Token::from("case");
        let mut exprs = Vec::new();

        let ex = parse_enum_expression("family:debian || family:redhat");
        exprs.push((gc.enum_list.canonify_expression(&gc, None, ex).unwrap(), Vec::new()));
        let result = gc.enum_list.evaluate(&gc, None, &exprs, case);
        assert!(result.is_err());
        if let Error::List(errs) = result.unwrap_err() {
            assert_eq!(errs.len(), 1);
        }

        let ex = parse_enum_expression("os:aix");
        exprs.push((gc.enum_list.canonify_expression(&gc, None, ex).unwrap(), Vec::new()));
        assert_eq!(gc.enum_list.evaluate(&gc, None, &exprs, case), Ok(()));

        let ex = parse_enum_expression(" family:redhat");
        exprs.push((gc.enum_list.canonify_expression(&gc, None, ex).unwrap(), Vec::new()));
        let result = gc.enum_list.evaluate(&gc, None, &exprs, case);
        assert!(result.is_err());
        if let Error::List(errs) = result.unwrap_err() {
            assert_eq!(errs.len(), 2);
        }
    }

    #[test]
    fn test_default() {
        let gc = init_tests();
        let case = Token::from("case");
        let mut exprs = Vec::new();

        let ex = parse_enum_expression("family:debian || family:redhat");
        exprs.push((gc.enum_list.canonify_expression(&gc, None, ex).unwrap(), Vec::new()));
        let result = gc.enum_list.evaluate(&gc, None, &exprs, case);
        assert!(result.is_err());
        if let Error::List(errs) = result.unwrap_err() {
            assert_eq!(errs.len(), 1);
        }

        let ex = parse_enum_expression("default");
        exprs.push((gc.enum_list.canonify_expression(&gc, None, ex).unwrap(), Vec::new()));
        assert_eq!(gc.enum_list.evaluate(&gc, None, &exprs, case), Ok(()));
    }

    #[test]
    fn test_mapping_check() {
        let GlobalContext { enum_list: mut e, .. } = init_tests();
        assert!(e.mapping_check().is_ok());
        assert!(add_enum_mapping(&mut e, "enum os ~> family2 { *->* }",).is_err());
        assert!(e.mapping_check().is_err());
    }
}
