// Right now the structure in module ast::* is extremely low-level, i.e., faithful to the original
// encoding (e.g. order of sections, order of types in Type section, width of LEB128 numbers etc.)
// This allows decoding-encoding to round-trip, but is tedious to work with for instrumentation.
// TODO Is round-trip/this "faithfulness" to the exact original representation necessary?
// Or should we only provide a high-level AST that logically captures everything but may be
// serialized differently than the original module?

// TODO Would this higher level Module/AST format be more convenient to work with?
// - no WithSize<T> or Leb128<T>
// - no explicit TypeIdx, all types are inlined and the Type section is built upon serialization
//   with a HashMap to still avoid type duplication, then all inlined types are replaced with idx
//   into the "HashMap".
// -> TODO We cannot get completely rid of *Idx, because globals, locals, functions can and must still
//    be referenced from code. Maybe we should thus still have Type section and TypeIdx explicitly available?
// - functions combines Function and Code section
// - table combines Table and Element (initialization of tables) section
// - memory combines Memory and Data (initialization of memory) section

// TODO "streaming AST" API: return Module {} after reading only the first 8 bytes, implement
// Iterator<Item = Section> for Module -> Module must somehow retain the reader to do so...

use std::marker::PhantomData;

struct HighLevelModule {
    start: Option<Idx<Function>>,

    imports: Vec<Import>,
    exports: Vec<Export>,

    globals: Vec<Global>,
    functions: Vec<Function>,

    table: Table,
    memory: Memory,

    custom_sections: Vec<Vec<u8>>,
}

pub struct Idx<T>(pub usize, PhantomData<T>);

pub struct Function {
    type_: FunctionType,
    locals: Vec<Local>,
    body: Expr,
}

type Local = ValType;

pub struct Import {
    module: String,
    name: String,
    type_: ImportType,
}

pub struct Export {
    name: String,
    type_: ExportType,
}

pub enum ImportType {
    Function(FunctionType),
    Table(TableType),
    Memory(MemoryType),
    Global(GlobalType),
}

pub enum ExportType {
    Function(Idx<Function>),
    Table(Idx<Table>),
    Memory(Idx<Memory>),
    Global(Idx<Global>),
}

pub struct Table {
    type_: TableType,
    inits: Vec<Element>,
}

pub struct Memory {
    type_: MemoryType,
    inits: Vec<Data>,
}

// == TableInit
pub struct Element {
    offset: ConstExpr,
    functions: Vec<Idx<Function>>,
}

// == MemoryInit
pub struct Data {
    offset: ConstExpr,
    bytes: Vec<u8>,
}

pub struct FunctionType(Vec<ValType>, Vec<ValType>);

pub struct TableType(ElemType, Limits);

pub enum ElemType {
    Anyfunc,
}

pub struct MemoryType(Limits);

pub struct Limits {
    pub initial_size: u32,
    pub max_size: Option<u32>,
}

pub struct Global {
    type_: GlobalType,
    init: ConstExpr,
}

pub struct GlobalType(ValType, Mutability);

pub enum ValType {
    I32,
    I64,
    F32,
    F64,
}

pub enum Mutability {
    Const,
    Mut,
}

pub struct Label;

pub type BlockType = Option<ValType>;
pub type Expr = Vec<Instr>;
pub type ConstExpr = Vec<Instr>;

pub enum Instr {
    Unreachable,
    Nop,

    Block(BlockType, Expr),
    Loop(BlockType, Expr),
    If(BlockType, Expr),
    Else(Expr),
    End,

    Br(Idx<Label>),
    BrIf(Idx<Label>),
    BrTable(Vec<Idx<Label>>, Idx<Label>),

    Return,
    Call(Idx<Function>),
    CallIndirect(FunctionType, /* unused, always 0x00 in WASM version 1 */ Idx<Table>),

    Drop,
    Select,

    GetLocal(Idx<Local>),
    SetLocal(Idx<Local>),
    TeeLocal(Idx<Local>),
    GetGlobal(Idx<Global>),
    SetGlobal(Idx<Global>),

    I32Load(Memarg),
    I64Load(Memarg),
    F32Load(Memarg),
    F64Load(Memarg),
    I32Load8S(Memarg),
    I32Load8U(Memarg),
    I32Load16S(Memarg),
    I32Load16U(Memarg),
    I64Load8S(Memarg),
    I64Load8U(Memarg),
    I64Load16S(Memarg),
    I64Load16U(Memarg),
    I64Load32S(Memarg),
    I64Load32U(Memarg),
    I32Store(Memarg),
    I64Store(Memarg),
    F32Store(Memarg),
    F64Store(Memarg),
    I32Store8(Memarg),
    I32Store16(Memarg),
    I64Store8(Memarg),
    I64Store16(Memarg),
    I64Store32(Memarg),

    CurrentMemory(/* unused, always 0x00 in WASM version 1 */ Idx<Memory>),
    GrowMemory(/* unused, always 0x00 in WASM version 1 */ Idx<Memory>),

    I32Const(i32),
    I64Const(i64),
    F32Const(f32),
    F64Const(f64),

    I32Eqz,
    I32Eq,
    I32Ne,
    I32LtS,
    I32LtU,
    I32GtS,
    I32GtU,
    I32LeS,
    I32LeU,
    I32GeS,
    I32GeU,
    I64Eqz,
    I64Eq,
    I64Ne,
    I64LtS,
    I64LtU,
    I64GtS,
    I64GtU,
    I64LeS,
    I64LeU,
    I64GeS,
    I64GeU,
    F32Eq,
    F32Ne,
    F32Lt,
    F32Gt,
    F32Le,
    F32Ge,
    F64Eq,
    F64Ne,
    F64Lt,
    F64Gt,
    F64Le,
    F64Ge,
    I32Clz,
    I32Ctz,
    I32Popcnt,
    I32Add,
    I32Sub,
    I32Mul,
    I32DivS,
    I32DivU,
    I32RemS,
    I32RemU,
    I32And,
    I32Or,
    I32Xor,
    I32Shl,
    I32ShrS,
    I32ShrU,
    I32Rotl,
    I32Rotr,
    I64Clz,
    I64Ctz,
    I64Popcnt,
    I64Add,
    I64Sub,
    I64Mul,
    I64DivS,
    I64DivU,
    I64RemS,
    I64RemU,
    I64And,
    I64Or,
    I64Xor,
    I64Shl,
    I64ShrS,
    I64ShrU,
    I64Rotl,
    I64Rotr,
    F32Abs,
    F32Neg,
    F32Ceil,
    F32Floor,
    F32Trunc,
    F32Nearest,
    F32Sqrt,
    F32Add,
    F32Sub,
    F32Mul,
    F32Div,
    F32Min,
    F32Max,
    F32Copysign,
    F64Abs,
    F64Neg,
    F64Ceil,
    F64Floor,
    F64Trunc,
    F64Nearest,
    F64Sqrt,
    F64Add,
    F64Sub,
    F64Mul,
    F64Div,
    F64Min,
    F64Max,
    F64Copysign,
    I32WrapI64,
    I32TruncSF32,
    I32TruncUF32,
    I32TruncSF64,
    I32TruncUF64,
    I64ExtendSI32,
    I64ExtendUI32,
    I64TruncSF32,
    I64TruncUF32,
    I64TruncSF64,
    I64TruncUF64,
    F32ConvertSI32,
    F32ConvertUI32,
    F32ConvertSI64,
    F32ConvertUI64,
    F32DemoteF64,
    F64ConvertSI32,
    F64ConvertUI32,
    F64ConvertSI64,
    F64ConvertUI64,
    F64PromoteF32,
    I32ReinterpretF32,
    I64ReinterpretF64,
    F32ReinterpretI32,
    F64ReinterpretI64,
}

pub struct Memarg {
    pub alignment: u32,
    pub offset: u32,
}