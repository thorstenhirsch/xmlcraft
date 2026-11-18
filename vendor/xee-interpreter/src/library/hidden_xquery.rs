// XmlCraft patch: functions XmlCraft's XQuery compiler (`xmlcraft-xquery`) compiles to, not meant to be called by name.

use xee_xpath_macros::xpath_fn;
use xot::xmlname::NameStrInfo;

use crate::atomic;
use crate::error;
use crate::function::StaticFunctionDescription;
use crate::interpreter::Interpreter;
use crate::sequence;
use crate::wrap_xpath_fn;

/// Errors in this namespace are XmlCraft's limits (a step budget, a stopped run): `try`/`catch` does not catch them.
const LIMIT_NAMESPACE: &str = "http://xmlcraft.org/ns/limit";

// XQuery 3.1 §3.17 `try { body } catch … { … }`: run `$body` (a function of no arguments that shares the context of the `try`);
// when it raises a dynamic error, put the interpreter back as it was before the call and return `$handler($code, $description,
// $value)`. The handler picks the `catch` clause and raises the error again when none matches. (The error object of `fn:error`
// is not kept by the engine, so `$value` is empty.)
#[xpath_fn("fn:xmlcraft-try($body as function(*), $handler as function(*)) as item()*")]
fn xmlcraft_try(
    interpreter: &mut Interpreter,
    body: sequence::Item,
    handler: sequence::Item,
) -> error::Result<sequence::Sequence> {
    let body = body.to_function()?;
    let handler = handler.to_function()?;
    let checkpoint = interpreter.checkpoint();
    let nested = interpreter.nested_calls;
    match interpreter.call_function_with_arguments(&body, &[]) {
        Ok(value) => Ok(value),
        Err(e) => {
            let qname = e.code_qname();
            // (The W3C codes get the `err` prefix, so that `$err:code` reads as the specifications write it.)
            let qname = if qname.namespace() == "http://www.w3.org/2005/xqt-errors" && qname.prefix().is_empty() {
                xot::xmlname::OwnedName::new(qname.local_name().to_string(), qname.namespace().to_string(), "err".to_string())
            } else {
                qname
            };
            if qname.namespace() == LIMIT_NAMESPACE || matches!(e, error::Error::StackOverflow) {
                return Err(e);
            }
            interpreter.restore(checkpoint);
            interpreter.nested_calls = nested;
            let description = e.message().to_string();
            let code: sequence::Sequence = atomic::Atomic::from(qname).into();
            let description: sequence::Sequence = atomic::Atomic::from(description).into();
            interpreter.call_function_with_arguments(
                &handler,
                &[code, description, sequence::Sequence::default()],
            )
        }
    }
}

pub(crate) fn static_function_descriptions() -> Vec<StaticFunctionDescription> {
    vec![wrap_xpath_fn!(xmlcraft_try)]
}
