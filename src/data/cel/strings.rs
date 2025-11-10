use cel_interpreter::extractors::{Arguments, This};
use cel_interpreter::{ExecutionError, ResolveResult, Value};
use std::sync::Arc;

pub fn char_at(This(this): This<Arc<String>>, arg: i64) -> ResolveResult {
    match this.chars().nth(arg as usize) {
        None => Err(ExecutionError::FunctionError {
            function: "String.charAt".to_owned(),
            message: format!("No index {arg} on `{this}`"),
        }),
        Some(c) => Ok(c.to_string().into()),
    }
}

pub fn index_of(
    This(this): This<Arc<String>>,
    arg: Arc<String>,
    Arguments(args): Arguments,
) -> ResolveResult {
    match args.len() {
        1 => match this.find(&*arg) {
            None => Ok((-1).into()),
            Some(idx) => Ok((idx as u64).into()),
        },
        2 => {
            let base = match args[1] {
                Value::Int(i) => i as usize,
                Value::UInt(u) => u as usize,
                _ => {
                    return Err(ExecutionError::FunctionError {
                        function: "String.indexOf".to_owned(),
                        message: format!(
                            "Expects 2nd argument to be an Integer, got `{:?}`",
                            args[1]
                        ),
                    })
                }
            };
            if base >= this.len() {
                return Ok((-1).into());
            }
            match this[base..].find(&*arg) {
                None => Ok((-1).into()),
                Some(idx) => Ok(Value::UInt((base + idx) as u64)),
            }
        }
        _ => Err(ExecutionError::FunctionError {
            function: "String.indexOf".to_owned(),
            message: format!("Expects 2 arguments at most, got `{args:?}`!"),
        }),
    }
}

pub fn last_index_of(
    This(this): This<Arc<String>>,
    arg: Arc<String>,
    Arguments(args): Arguments,
) -> ResolveResult {
    match args.len() {
        1 => match this.rfind(&*arg) {
            None => Ok((-1).into()),
            Some(idx) => Ok((idx as u64).into()),
        },
        2 => {
            let base = match args[1] {
                Value::Int(i) => i as usize,
                Value::UInt(u) => u as usize,
                _ => {
                    return Err(ExecutionError::FunctionError {
                        function: "String.lastIndexOf".to_owned(),
                        message: format!(
                            "Expects 2nd argument to be an Integer, got `{:?}`",
                            args[1]
                        ),
                    })
                }
            };
            if base >= this.len() {
                return Ok((-1).into());
            }
            match this[base..].rfind(&*arg) {
                None => Ok((-1).into()),
                Some(idx) => Ok(Value::UInt(idx as u64)),
            }
        }
        _ => Err(ExecutionError::FunctionError {
            function: "String.lastIndexOf".to_owned(),
            message: format!("Expects 2 arguments at most, got `{args:?}`!"),
        }),
    }
}

pub fn join(This(this): This<Arc<Vec<Value>>>, Arguments(args): Arguments) -> ResolveResult {
    let separator = args
        .first()
        .map(|v| match v {
            Value::String(s) => Ok(s.as_str()),
            _ => Err(ExecutionError::FunctionError {
                function: "List.join".to_owned(),
                message: format!("Expects seperator to be a String, got `{v:?}`!"),
            }),
        })
        .unwrap_or(Ok(""))?;
    Ok(this
        .iter()
        .map(|v| match v {
            Value::String(s) => Ok(s.as_str().to_string()),
            _ => Err(ExecutionError::FunctionError {
                function: "List.join".to_owned(),
                message: "Expects a list of String values!".to_owned(),
            }),
        })
        .collect::<Result<Vec<_>, _>>()?
        .join(separator)
        .into())
}

pub fn lower_ascii(This(this): This<Arc<String>>) -> ResolveResult {
    Ok(this.to_ascii_lowercase().into())
}

pub fn upper_ascii(This(this): This<Arc<String>>) -> ResolveResult {
    Ok(this.to_ascii_uppercase().into())
}

pub fn trim(This(this): This<Arc<String>>) -> ResolveResult {
    Ok(this.trim().into())
}

pub fn replace(This(this): This<Arc<String>>, Arguments(args): Arguments) -> ResolveResult {
    match args.len() {
        count @ 2..=3 => {
            let from = match &args[0] {
                Value::String(s) => s.as_str(),
                _ => Err(ExecutionError::FunctionError {
                    function: "String.replace".to_owned(),
                    message: format!(
                        "First argument of type String expected, got `{:?}`",
                        args[0]
                    ),
                })?,
            };
            let to = match &args[1] {
                Value::String(s) => s.as_str(),
                _ => Err(ExecutionError::FunctionError {
                    function: "String.replace".to_owned(),
                    message: format!(
                        "Second argument of type String expected, got `{:?}`",
                        args[1]
                    ),
                })?,
            };
            if count == 3 {
                let n = match &args[2] {
                    Value::Int(i) => *i as usize,
                    Value::UInt(u) => *u as usize,
                    _ => Err(ExecutionError::FunctionError {
                        function: "String.replace".to_owned(),
                        message: format!(
                            "Third argument of type Integer expected, got `{:?}`",
                            args[2]
                        ),
                    })?,
                };
                Ok(this.replacen(from, to, n).into())
            } else {
                Ok(this.replace(from, to).into())
            }
        }
        _ => Err(ExecutionError::FunctionError {
            function: "String.replace".to_owned(),
            message: format!("Expects 2 or 3 arguments, got {args:?}"),
        }),
    }
}

pub fn split(This(this): This<Arc<String>>, Arguments(args): Arguments) -> ResolveResult {
    match args.len() {
        count @ 1..=2 => {
            let sep = match &args[0] {
                Value::String(sep) => sep.as_str(),
                _ => {
                    return Err(ExecutionError::FunctionError {
                        function: "String.split".to_string(),
                        message: format!(
                            "Expects a first argument of type String, got `{:?}`",
                            args[0]
                        ),
                    })
                }
            };
            let list = if count == 2 {
                let pos = match &args[1] {
                    Value::UInt(u) => *u as usize,
                    Value::Int(i) => *i as usize,
                    _ => Err(ExecutionError::FunctionError {
                        function: "String.split".to_string(),
                        message: format!(
                            "Expects a second argument of type Integer, got `{:?}`",
                            args[1]
                        ),
                    })?,
                };
                this.splitn(pos, sep)
                    .map(|s| Value::String(s.to_owned().into()))
                    .collect::<Vec<Value>>()
            } else {
                this.split(sep)
                    .map(|s| Value::String(s.to_owned().into()))
                    .collect::<Vec<Value>>()
            };
            Ok(list.into())
        }
        _ => Err(ExecutionError::FunctionError {
            function: "String.split".to_owned(),
            message: format!("Expects at most 2 arguments, got {args:?}"),
        }),
    }
}

pub fn substring(This(this): This<Arc<String>>, Arguments(args): Arguments) -> ResolveResult {
    match args.len() {
        count @ 1..=2 => {
            let start = match &args[0] {
                Value::Int(i) => *i as usize,
                Value::UInt(u) => *u as usize,
                _ => Err(ExecutionError::FunctionError {
                    function: "String.substring".to_string(),
                    message: format!(
                        "Expects a first argument of type Integer, got `{:?}`",
                        args[0]
                    ),
                })?,
            };
            let end = if count == 2 {
                match &args[1] {
                    Value::Int(i) => *i as usize,
                    Value::UInt(u) => *u as usize,
                    _ => Err(ExecutionError::FunctionError {
                        function: "String.substring".to_string(),
                        message: format!(
                            "Expects a second argument of type Integer, got `{:?}`",
                            args[0]
                        ),
                    })?,
                }
            } else {
                this.chars().count()
            };
            if end < start {
                Err(ExecutionError::FunctionError {
                    function: "String.substring".to_string(),
                    message: format!("Can't have end be before the start: `{end} < {start}"),
                })?
            }
            Ok(this
                .chars()
                .skip(start)
                .take(end - start)
                .collect::<String>()
                .into())
        }
        _ => Err(ExecutionError::FunctionError {
            function: "String.substring".to_owned(),
            message: format!("Expects at most 2 arguments, got {args:?}"),
        }),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::data::{attribute::AttributeState, cel::Expression};
    use crate::kuadrant::{MockWasmHost, ReqRespCtx};
    use cel_interpreter::Value;

    #[test]
    fn extended_string_fn() {
        let ctx = ReqRespCtx::new(Arc::new(MockWasmHost::new()));

        let e = Expression::new("'abc'.charAt(1)").expect("This must be valid CEL");
        assert_eq!(e.eval(&ctx), Ok(AttributeState::Available("b".into())));

        let e = Expression::new("'hello mellow'.indexOf('')").expect("This must be valid CEL");
        assert_eq!(e.eval(&ctx), Ok(AttributeState::Available(0.into())));
        let e = Expression::new("'hello mellow'.indexOf('ello')").expect("This must be valid CEL");
        assert_eq!(e.eval(&ctx), Ok(AttributeState::Available(1.into())));
        let e = Expression::new("'hello mellow'.indexOf('jello')").expect("This must be valid CEL");
        assert_eq!(e.eval(&ctx), Ok(AttributeState::Available((-1).into())));
        let e = Expression::new("'hello mellow'.indexOf('', 2)").expect("This must be valid CEL");
        assert_eq!(e.eval(&ctx), Ok(AttributeState::Available(2.into())));
        let e =
            Expression::new("'hello mellow'.indexOf('ello', 20)").expect("This must be valid CEL");
        assert_eq!(e.eval(&ctx), Ok(AttributeState::Available((-1).into())));

        let e = Expression::new("'hello mellow'.lastIndexOf('')").expect("This must be valid CEL");
        assert_eq!(e.eval(&ctx), Ok(AttributeState::Available(12.into())));
        let e =
            Expression::new("'hello mellow'.lastIndexOf('ello')").expect("This must be valid CEL");
        assert_eq!(e.eval(&ctx), Ok(AttributeState::Available(7.into())));
        let e =
            Expression::new("'hello mellow'.lastIndexOf('jello')").expect("This must be valid CEL");
        assert_eq!(e.eval(&ctx), Ok(AttributeState::Available((-1).into())));
        let e = Expression::new("'hello mellow'.lastIndexOf('ello', 6)")
            .expect("This must be valid CEL");
        assert_eq!(e.eval(&ctx), Ok(AttributeState::Available(1.into())));
        let e = Expression::new("'hello mellow'.lastIndexOf('ello', 20)")
            .expect("This must be valid CEL");
        assert_eq!(e.eval(&ctx), Ok(AttributeState::Available((-1).into())));

        let e = Expression::new("['hello', 'mellow'].join()").expect("This must be valid CEL");
        assert_eq!(
            e.eval(&ctx),
            Ok(AttributeState::Available("hellomellow".into()))
        );
        let e = Expression::new("[].join()").expect("This must be valid CEL");
        assert_eq!(e.eval(&ctx), Ok(AttributeState::Available("".into())));
        let e = Expression::new("['hello', 'mellow'].join(' ')").expect("This must be valid CEL");
        assert_eq!(
            e.eval(&ctx),
            Ok(AttributeState::Available("hello mellow".into()))
        );

        let e = Expression::new("'TacoCat'.lowerAscii()").expect("This must be valid CEL");
        assert_eq!(
            e.eval(&ctx),
            Ok(AttributeState::Available("tacocat".into()))
        );
        let e = Expression::new("'TacoCÆt Xii'.lowerAscii()").expect("This must be valid CEL");
        assert_eq!(
            e.eval(&ctx),
            Ok(AttributeState::Available("tacocÆt xii".into()))
        );

        let e = Expression::new("'TacoCat'.upperAscii()").expect("This must be valid CEL");
        assert_eq!(
            e.eval(&ctx),
            Ok(AttributeState::Available("TACOCAT".into()))
        );
        let e = Expression::new("'TacoCÆt Xii'.upperAscii()").expect("This must be valid CEL");
        assert_eq!(
            e.eval(&ctx),
            Ok(AttributeState::Available("TACOCÆT XII".into()))
        );

        let e = Expression::new("'  \ttrim\n    '.trim()").expect("This must be valid CEL");
        assert_eq!(e.eval(&ctx), Ok(AttributeState::Available("trim".into())));

        let e =
            Expression::new("'hello hello'.replace('he', 'we')").expect("This must be valid CEL");
        assert_eq!(
            e.eval(&ctx),
            Ok(AttributeState::Available("wello wello".into()))
        );
        let e = Expression::new("'hello hello'.replace('he', 'we', -1)")
            .expect("This must be valid CEL");
        assert_eq!(
            e.eval(&ctx),
            Ok(AttributeState::Available("wello wello".into()))
        );
        let e = Expression::new("'hello hello'.replace('he', 'we', 1)")
            .expect("This must be valid CEL");
        assert_eq!(
            e.eval(&ctx),
            Ok(AttributeState::Available("wello hello".into()))
        );
        let e = Expression::new("'hello hello'.replace('he', 'we', 0)")
            .expect("This must be valid CEL");
        assert_eq!(
            e.eval(&ctx),
            Ok(AttributeState::Available("hello hello".into()))
        );
        let e = Expression::new("'hello hello'.replace('', '_')").expect("This must be valid CEL");
        assert_eq!(
            e.eval(&ctx),
            Ok(AttributeState::Available("_h_e_l_l_o_ _h_e_l_l_o_".into()))
        );
        let e = Expression::new("'hello hello'.replace('h', '')").expect("This must be valid CEL");
        assert_eq!(
            e.eval(&ctx),
            Ok(AttributeState::Available("ello ello".into()))
        );

        let e = Expression::new("'hello hello hello'.split(' ')").expect("This must be valid CEL");
        assert_eq!(
            e.eval(&ctx),
            Ok(AttributeState::Available(
                vec!["hello", "hello", "hello"].into()
            ))
        );
        let e =
            Expression::new("'hello hello hello'.split(' ', 0)").expect("This must be valid CEL");
        assert_eq!(
            e.eval(&ctx),
            Ok(AttributeState::Available(Value::List(vec![].into())))
        );
        let e =
            Expression::new("'hello hello hello'.split(' ', 1)").expect("This must be valid CEL");
        assert_eq!(
            e.eval(&ctx),
            Ok(AttributeState::Available(vec!["hello hello hello"].into()))
        );
        let e =
            Expression::new("'hello hello hello'.split(' ', 2)").expect("This must be valid CEL");
        assert_eq!(
            e.eval(&ctx),
            Ok(AttributeState::Available(
                vec!["hello", "hello hello"].into()
            ))
        );
        let e =
            Expression::new("'hello hello hello'.split(' ', -1)").expect("This must be valid CEL");
        assert_eq!(
            e.eval(&ctx),
            Ok(AttributeState::Available(
                vec!["hello", "hello", "hello"].into()
            ))
        );

        let e = Expression::new("'tacocat'.substring(4)").expect("This must be valid CEL");
        assert_eq!(e.eval(&ctx), Ok(AttributeState::Available("cat".into())));
        let e = Expression::new("'tacocat'.substring(0, 4)").expect("This must be valid CEL");
        assert_eq!(e.eval(&ctx), Ok(AttributeState::Available("taco".into())));
        let e = Expression::new("'ta©o©αT'.substring(2, 6)").expect("This must be valid CEL");
        assert_eq!(e.eval(&ctx), Ok(AttributeState::Available("©o©α".into())));
    }
}
