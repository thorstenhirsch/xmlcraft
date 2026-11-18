//! XmlCraft patch: a hook the interpreter calls before each instruction, for XmlCraft's XPath profiler and debugger.
//!
//! Without a hook the interpreter checks one `Option` per instruction and nothing else changes. With one, [`Hook::before`] is called
//! before every instruction with a read-only view of the interpreter ([`At`]): the function and the place in it, the source span the
//! compiler gave the instruction, the frames, the stack, the sequences being built and the documents. An error returned by the hook
//! stops the run with that error (a debugger stopping, a profiler over its budget).

use xot::xmlname::NameStrInfo;
use xot::Xot;

use crate::context::DynamicContext;
use crate::error;
use crate::function;
use crate::sequence;
use crate::span::SourceSpan;
use crate::stack;

use super::instruction::{decode_instruction, Instruction};
use super::interpret::Interpreter;
use super::program::Program;

/// What is told about each instruction before it runs.
pub trait Hook {
    /// Called before each instruction. An error stops the run with it.
    fn before(&mut self, at: &At<'_, '_>) -> error::Result<()>;
}

/// A read-only view of the interpreter, before an instruction runs.
pub struct At<'i, 'a> {
    pub(crate) interpreter: &'i Interpreter<'a>,
}

/// One frame of the interpreter: a call of an inline function (the outermost is the expression itself).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameView {
    /// The function, as an index into [`Program::functions`].
    pub function: usize,
    /// The next instruction of the frame (for a frame below the top, the one after its call).
    pub ip: usize,
    /// Where the frame's locals start on the stack.
    pub base: usize,
}

impl<'i, 'a> At<'i, 'a> {
    /// The function being run, as an index into [`Program::functions`].
    pub fn function(&self) -> usize {
        self.interpreter.state.frame().function().0
    }

    /// The place of the instruction about to run in its function's code.
    pub fn ip(&self) -> usize {
        self.interpreter.state.frame().ip
    }

    /// How many frames there are (1 for the expression itself, one more for each inline function called).
    pub fn depth(&self) -> usize {
        self.interpreter.state.frames().len()
    }

    /// The frames, the outermost first.
    pub fn frames(&self) -> Vec<FrameView> {
        self.interpreter
            .state
            .frames()
            .iter()
            .map(|f| FrameView {
                function: f.function().0,
                ip: f.ip,
                base: f.base(),
            })
            .collect()
    }

    /// The source span the compiler gave the instruction about to run.
    pub fn span(&self) -> Option<SourceSpan> {
        let function = self.current_function()?;
        function.spans.get(self.ip()).copied()
    }

    /// The source span of an instruction of a function.
    pub fn span_of(&self, function: usize, ip: usize) -> Option<SourceSpan> {
        self.program().functions.get(function)?.spans.get(ip).copied()
    }

    /// The instruction about to run.
    pub fn instruction(&self) -> Option<Instruction> {
        let function = self.current_function()?;
        let bytes = function.chunk.get(self.ip()..)?;
        if bytes.is_empty() {
            return None;
        }
        Some(decode_instruction(bytes).0)
    }

    /// The whole stack, the bottom first.
    pub fn stack(&self) -> &[stack::Value] {
        self.interpreter.state.stack()
    }

    /// Where the locals of the current frame start on the stack.
    pub fn base(&self) -> usize {
        self.interpreter.state.frame().base()
    }

    /// The sequences being built (by a path step, a predicate, a `for`), the outermost first: the items each has so far.
    pub fn builds(&self) -> Vec<&[sequence::Item]> {
        self.interpreter.state.builds()
    }

    /// A value the current inline function closed over.
    pub fn closure_value(&self, index: usize) -> Option<stack::Value> {
        if self.depth() <= 1 {
            return None;
        }
        let function = self.interpreter.state.function().ok()?;
        match function {
            function::Function::Inline(data) => data.closure_vars.get(index).cloned(),
            function::Function::Static(data) => data.closure_vars.get(index).cloned(),
            _ => None,
        }
    }

    /// The name of the function a `Call` with this many arguments is about to call, as (namespace, local name): a static function's
    /// (`count` in the `fn` namespace); none for an inline function, a map or an array.
    pub fn callee_name(&self, arity: usize) -> Option<(String, String)> {
        let stack = self.stack();
        let at = stack.len().checked_sub(arity.checked_add(1)?)?;
        let value = stack.get(at)?;
        let stack::Value::Sequence(sequence) = value else {
            return None;
        };
        let function: function::Function = sequence.clone().try_into().ok()?;
        let name = self.interpreter.runnable().function_info(&function).name()?;
        Some((name.namespace().to_owned(), name.local_name().to_owned()))
    }

    /// The documents.
    pub fn xot(&self) -> &Xot {
        self.interpreter.state.xot()
    }

    /// The program being run.
    pub fn program(&self) -> &Program {
        self.interpreter.runnable().program()
    }

    /// The dynamic context of the run (to write maps, arrays and functions with).
    pub fn dynamic_context(&self) -> &DynamicContext {
        self.interpreter.runnable().dynamic_context()
    }

    fn current_function(&self) -> Option<&function::InlineFunction> {
        self.program().functions.get(self.function())
    }
}
