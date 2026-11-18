/// The core of the interpreter: bytecodes and a way to run them. Bytecodes
/// are contained in functions, which together are composed into a program.
// XmlCraft patch: the hook of the profiler and debugger.
pub mod hook;
pub mod instruction;
mod interpret;
mod program;
mod runnable;
mod state;

pub use interpret::Interpreter;
pub use program::{FunctionInfo, Program};
pub use runnable::Runnable;
