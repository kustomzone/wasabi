use ast::{FunctionType, GlobalType, Idx, Label, Limits, Local, Memarg, MemoryType, Mutability, ValType, ValType::*};
use ast::highlevel::{Code, Expr, Function, Instr, Instr::*, InstrGroup, InstrGroup::*, Memory, Module};
use std::collections::{HashMap, HashSet};
use std::mem::{discriminant, Discriminant};
use super::convert_i64::{convert_i64_instr, convert_i64_type};
use super::js_codegen::{append_mangled_tys, js_codegen};
use super::static_info::*;
use super::type_stack::TypeStack;

/// instruments every instruction in Jalangi-style with a callback that takes inputs, outputs, other
/// relevant information.
pub fn add_hooks(module: &mut Module) -> Option<String> {
    // export the table for the JS code to translate table indices -> function indices
    for table in &mut module.tables {
        if let None = table.export {
            table.export = Some("table".into());
        }
    }

    let mut module_info: ModuleInfo = (&*module).into();
    // TODO use something more meaningful than Strings, e.g., a LowlevelHook struct or so...
    let mut on_demand_hooks = Vec::new();

    /* add hooks (imported functions, provided by the analysis in JavaScript) */

    // polymorphic hooks:
    // - 1 instruction : N hooks
    // - instruction can take stack arguments/produce results of several types
    // - we need to "monomorphize", i.e., create one hook per occurring polymorphic type
    let mut polymorphic_hooks = PolymorphicHookMap::new();

    // collect some info, necessary for monomorphization of polymorphic hooks
    let (mut unique_arg_tys, mut unique_result_tys): (Vec<Vec<ValType>>, Vec<Vec<ValType>>) = module.functions.iter()
        .map(|func| (func.type_.params.clone(), func.type_.results.clone()))
        .unzip();
    unique_result_tys.sort();
    unique_result_tys.dedup();
    unique_arg_tys.sort();
    unique_arg_tys.dedup();

    // returns
    polymorphic_hooks.add(module, Return, &[], unique_result_tys.as_slice(), &mut on_demand_hooks);

    // locals and globals
    let primitive_tys = &[vec![I32], vec![I64], vec![F32], vec![F64]];
    polymorphic_hooks.add(module, GetLocal(0.into()), &[I32], primitive_tys, &mut on_demand_hooks);
    polymorphic_hooks.add(module, SetLocal(0.into()), &[I32], primitive_tys, &mut on_demand_hooks);
    polymorphic_hooks.add(module, TeeLocal(0.into()), &[I32], primitive_tys, &mut on_demand_hooks);
    polymorphic_hooks.add(module, GetGlobal(0.into()), &[I32], primitive_tys, &mut on_demand_hooks);
    polymorphic_hooks.add(module, SetGlobal(0.into()), &[I32], primitive_tys, &mut on_demand_hooks);

    // drop and select
    polymorphic_hooks.add(module, Drop, &[], primitive_tys, &mut on_demand_hooks);
    polymorphic_hooks.add(module, Select, &[I32], &[vec![I32, I32], vec![I64, I64], vec![F32, F32], vec![F64, F64]], &mut on_demand_hooks);

    // calls
    polymorphic_hooks.add(module, Call(0.into()), &[I32], unique_arg_tys.as_slice(), &mut on_demand_hooks); // I32 = target func idx
    polymorphic_hooks.add(module, CallIndirect(FunctionType::new(vec![], vec![]), 0.into()), &[I32], unique_arg_tys.as_slice(), &mut on_demand_hooks); // I32 = target table idx
    // manually add call_post hook since it does not directly correspond to an instruction
    let call_result_hooks: HashMap<&[ValType], Idx<Function>> = unique_result_tys.iter()
        .map(|tys| {
            let tys = tys.as_slice();
            (tys, add_hook(module, append_mangled_tys("call_result".into(), tys), tys))
        }).collect();

    // monomorphic hooks:
    // - 1 hook : 1 instruction
    // - argument/result types are directly determined from the instruction itself
    let if_hook = add_hook(module, "if_", &[/* condition */ I32]);
    // [I32, I32] for label and target instruction index (determined statically)
    let br_hook = add_hook(module, "br", &[I32, I32]);
    let br_if_hook = add_hook(module, "br_if", &[/* condition */ I32, /* target label and instr */ I32, I32]);
    let br_table_hook = add_hook(module, "br_table", &[/* br_table_info_idx */ I32, /* table_idx */ I32]);

    // all end hooks also give the instruction index of the corresponding begin (except for functions,
    // where it implicitly is -1 anyway)
    let begin_function_hook = add_hook(module, "begin_function", &[]);
    let end_function_hook = add_hook(module, "end_function", &[]);
    let begin_block_hook = add_hook(module, "begin_block", &[]);
    let end_block_hook = add_hook(module, "end_block", &[I32]);
    let begin_loop_hook = add_hook(module, "begin_loop", &[]);
    let end_loop_hook = add_hook(module, "end_loop", &[I32]);
    let begin_if_hook = add_hook(module, "begin_if", &[]);
    let end_if_hook = add_hook(module, "end_if", &[I32]);
    let begin_else_hook = add_hook(module, "begin_else", &[]);
    let end_else_hook = add_hook(module, "end_else", &[I32]);

    let nop_hook = add_hook(module, "nop", &[]);
    let unreachable_hook = add_hook(module, "unreachable", &[]);

    let current_memory_hook = add_hook(module, "current_memory", &[I32]);
    let grow_memory_hook = add_hook(module, "grow_memory", &[I32, I32]);

    // TODO make this a struct of its own, similar to PolymorphicHookMap
    let monomorphic_hook_call = {
        let monomorphic_hooks: HashMap<Discriminant<Instr>, Idx<Function>> = [
            I32Const(0),
            I64Const(0),
            F32Const(0.0),
            F64Const(0.0),

            // Unary
            I32Eqz, I64Eqz,
            I32Clz, I32Ctz, I32Popcnt,
            I64Clz, I64Ctz, I64Popcnt,
            F32Abs, F32Neg, F32Ceil, F32Floor, F32Trunc, F32Nearest, F32Sqrt,
            F64Abs, F64Neg, F64Ceil, F64Floor, F64Trunc, F64Nearest, F64Sqrt,
            I32WrapI64,
            I32TruncSF32, I32TruncUF32,
            I32TruncSF64, I32TruncUF64,
            I64ExtendSI32, I64ExtendUI32,
            I64TruncSF32, I64TruncUF32,
            I64TruncSF64, I64TruncUF64,
            F32ConvertSI32, F32ConvertUI32,
            F32ConvertSI64, F32ConvertUI64,
            F32DemoteF64,
            F64ConvertSI32, F64ConvertUI32,
            F64ConvertSI64, F64ConvertUI64,
            F64PromoteF32,
            I32ReinterpretF32,
            I64ReinterpretF64,
            F32ReinterpretI32,
            F64ReinterpretI64,

            // Binary
            I32Eq, I32Ne, I32LtS, I32LtU, I32GtS, I32GtU, I32LeS, I32LeU, I32GeS, I32GeU,
            I64Eq, I64Ne, I64LtS, I64LtU, I64GtS, I64GtU, I64LeS, I64LeU, I64GeS, I64GeU,
            F32Eq, F32Ne, F32Lt, F32Gt, F32Le, F32Ge,
            F64Eq, F64Ne, F64Lt, F64Gt, F64Le, F64Ge,
            I32Add, I32Sub, I32Mul, I32DivS, I32DivU, I32RemS, I32RemU, I32And, I32Or, I32Xor, I32Shl, I32ShrS, I32ShrU, I32Rotl, I32Rotr,
            I64Add, I64Sub, I64Mul, I64DivS, I64DivU, I64RemS, I64RemU, I64And, I64Or, I64Xor, I64Shl, I64ShrS, I64ShrU, I64Rotl, I64Rotr,
            F32Add, F32Sub, F32Mul, F32Div, F32Min, F32Max, F32Copysign,
            F64Add, F64Sub, F64Mul, F64Div, F64Min, F64Max, F64Copysign,

            // Memory
            I32Load(Memarg::default()), I32Load8S(Memarg::default()), I32Load8U(Memarg::default()), I32Load16S(Memarg::default()), I32Load16U(Memarg::default()),
            I64Load(Memarg::default()), I64Load8S(Memarg::default()), I64Load8U(Memarg::default()), I64Load16S(Memarg::default()), I64Load16U(Memarg::default()), I64Load32S(Memarg::default()), I64Load32U(Memarg::default()),
            F32Load(Memarg::default()),
            F64Load(Memarg::default()),
            I32Store(Memarg::default()), I32Store8(Memarg::default()), I32Store16(Memarg::default()),
            I64Store(Memarg::default()), I64Store8(Memarg::default()), I64Store16(Memarg::default()), I64Store32(Memarg::default()),
            F32Store(Memarg::default()),
            F64Store(Memarg::default()),
        ].into_iter()
            .map(|i| add_hook_from_instr(module, i, &mut on_demand_hooks))
            .collect();

        move |instr: &Instr| -> Instr {
            Call(*monomorphic_hooks
                .get(&discriminant(instr))
                .expect(&format!("no hook was added for instruction {}", instr.to_instr_name())))
        }
    };

    /* add call to hooks: setup code that copies the returned value, instruction location, call */
    // NOTE we do not need to filter out functions since all hooks are imports and thus won't have
    // Code to instrument anyway...
    for (fidx, function) in module.functions() {
        // only instrument non-imported functions
        if function.code.is_none() {
            continue;
        }

        // move body out of function, so that function is not borrowed during iteration over the original body
        let original_body = {
            let dummy_body = Vec::new();
            ::std::mem::replace(&mut function.code.as_mut().unwrap().body, dummy_body)
        };

        // allocate new instrumented body (i.e., do not modify in-place), since there are too many insertions anyway
        // there are at least 3 new instructions per original one (2 const for location + 1 hook call)
        let mut instrumented_body = Vec::with_capacity(4 * original_body.len());

        let mut block_stack = vec![Begin::Function];
        let mut type_stack = TypeStack::new();

        // add function_begin hook...
        instrumented_body.extend_from_slice(&[
            I32Const(fidx.0 as i32),
            // ...which does not correspond to any instruction, so take -1 as instruction index
            I32Const(-1),
            Call(begin_function_hook)
        ]);

        for (iidx, instr) in original_body.into_iter().enumerate() {
            let location = (I32Const(fidx.0 as i32), I32Const(iidx as i32));
            match (instr.group(), instr.clone()) {
                (_, Nop) | (_, Unreachable) => instrumented_body.extend_from_slice(&[
                    instr.clone(),
                    location.0,
                    location.1,
                    Call(match instr {
                        Nop => nop_hook,
                        Unreachable => unreachable_hook,
                        _ => unreachable!()
                    })]),
                (_, Block(ty)) | (_, Loop(ty)) => {
                    block_stack.push(match instr {
                        Block(_) => Begin::Block(iidx),
                        Loop(_) => Begin::Loop(iidx),
                        _ => unreachable!()
                    });
                    type_stack.begin_block(ty);

                    instrumented_body.extend_from_slice(&[
                        instr,
                        location.0,
                        location.1,
                        Call(begin_block_hook),
                    ]);
                }
                (_, If(ty)) => {
                    block_stack.push(Begin::If(iidx));
                    type_stack.begin_block(ty);

                    let condition_tmp = function.add_fresh_local(I32);

                    instrumented_body.extend_from_slice(&[
                        // if_ hook for the condition (always executed on either branch)
                        TeeLocal(condition_tmp),
                        location.0.clone(),
                        location.1.clone(),
                        GetLocal(condition_tmp),
                        Call(if_hook),
                        // actual if block start
                        instr,
                        // begin hook (not executed when condition implies else branch)
                        location.0,
                        location.1,
                        Call(begin_if_hook),
                    ]);
                }
                (_, Else) => {
                    let begin = block_stack.pop()
                        .expect(&format!("invalid begin/end nesting in function {}!", fidx.0));
                    if let Begin::If(begin_iidx) = begin {
                        block_stack.push(Begin::Else(iidx));
                        let block_ty = type_stack.end_block();
                        type_stack.begin_block(block_ty);

                        instrumented_body.extend_from_slice(&[
                            location.0.clone(),
                            location.1.clone(),
                            I32Const(begin_iidx as i32),
                            Call(end_else_hook),
                            instr,
                            location.0,
                            location.1,
                            Call(begin_else_hook),
                        ]);
                    } else {
                        unreachable!("else instruction should end if block, but was {:?}", begin);
                    }
                }
                (_, End) => {
                    let begin = block_stack.pop()
                        .expect(&format!("invalid begin/end nesting in function {}!", fidx.0));
                    // TODO better: add begin_function and end_function or so to type_stack
                    if begin != Begin::Function {
                        type_stack.end_block();
                    }

                    instrumented_body.extend_from_slice(&[
                        location.0,
                        location.1,
                    ]);
                    instrumented_body.append(&mut match begin {
                        Begin::Function => vec![Call(end_function_hook)],
                        Begin::Block(begin_iidx) => vec![I32Const(begin_iidx as i32), Call(end_block_hook)],
                        Begin::Loop(begin_iidx) => vec![I32Const(begin_iidx as i32), Call(end_loop_hook)],
                        Begin::If(begin_iidx) => vec![I32Const(begin_iidx as i32), Call(end_if_hook)],
                        Begin::Else(begin_iidx) => vec![I32Const(begin_iidx as i32), Call(end_else_hook)],
                    });
                    instrumented_body.push(instr);
                }
                (_, Drop) => {
                    let ty = type_stack.pop();
                    let tmp = function.add_fresh_local(ty);

                    instrumented_body.extend_from_slice(&[
                        SetLocal(tmp),
                        location.0,
                        location.1,
                    ]);
                    instrumented_body.append(&mut convert_i64_instr(GetLocal(tmp), ty));
                    instrumented_body.push(polymorphic_hooks.get_call(&instr, vec![ty]));
                }
                (_, Select) => {
                    assert_eq!(type_stack.pop(), I32, "select condition should be i32");
                    let ty = type_stack.pop();
                    assert_eq!(type_stack.pop(), ty, "select arguments should have same type");
                    type_stack.push(ty);

                    let condition_tmp = function.add_fresh_local(I32);
                    let arg_tmps = function.add_fresh_locals(&[ty, ty]);

                    instrumented_body.append(&mut save_stack_to_locals(&[arg_tmps[0], arg_tmps[1], condition_tmp]));
                    instrumented_body.extend_from_slice(&[
                        instr.clone(),
                        location.0,
                        location.1,
                        GetLocal(condition_tmp),
                    ]);
                    instrumented_body.append(&mut restore_locals_with_i64_handling(&arg_tmps, &[ty, ty]));
                    instrumented_body.push(polymorphic_hooks.get_call(&instr, vec![ty, ty]));
                }
                (_, CurrentMemory(_ /* memory idx == 0 in WASM version 1 */)) => {
                    type_stack.op(&[], &[I32]);

                    let result_tmp = function.add_fresh_local(I32);

                    instrumented_body.extend_from_slice(&[
                        instr,
                        TeeLocal(result_tmp),
                        location.0,
                        location.1,
                        GetLocal(result_tmp),
                        Call(current_memory_hook)
                    ]);
                }
                (_, GrowMemory(_ /* memory idx == 0 in WASM version 1 */)) => {
                    type_stack.op(&[I32], &[I32]);

                    let input_tmp = function.add_fresh_local(I32);
                    let result_tmp = function.add_fresh_local(I32);

                    instrumented_body.extend_from_slice(&[
                        TeeLocal(input_tmp),
                        instr,
                        TeeLocal(result_tmp),
                        location.0,
                        location.1,
                        GetLocal(input_tmp),
                        GetLocal(result_tmp),
                        Call(grow_memory_hook)
                    ]);
                }
                (_, GetLocal(local_idx)) | (_, SetLocal(local_idx)) | (_, TeeLocal(local_idx)) => {
                    let local_ty = function.local_type(local_idx);

                    match instr {
                        GetLocal(_) => type_stack.op(&[], &[local_ty]),
                        | SetLocal(_) => type_stack.op(&[local_ty], &[]),
                        _ => {}
                    }

                    instrumented_body.extend_from_slice(&[
                        instr.clone(),
                        location.0,
                        location.1,
                        I32Const(local_idx.0 as i32),
                    ]);
                    instrumented_body.append(&mut convert_i64_instr(GetLocal(local_idx), local_ty));
                    instrumented_body.push(polymorphic_hooks.get_call(&instr, vec![local_ty]));
                }
                (_, GetGlobal(global_idx)) | (_, SetGlobal(global_idx)) => {
                    let global_ty = module_info.globals[global_idx.0];

                    match instr {
                        GetGlobal(_) => type_stack.op(&[], &[global_ty]),
                        | SetGlobal(_) => type_stack.op(&[global_ty], &[]),
                        _ => {}
                    }

                    instrumented_body.extend_from_slice(&[
                        instr.clone(),
                        location.0,
                        location.1,
                        I32Const(global_idx.0 as i32),
                    ]);
                    instrumented_body.append(&mut convert_i64_instr(GetGlobal(global_idx), global_ty));
                    instrumented_body.push(polymorphic_hooks.get_call(&instr, vec![global_ty]));
                }
                (_, Return) => {
                    // FIXME type checking for return correctly handled?

                    let result_tys = function.type_.results.clone();
                    let result_tmps = function.add_fresh_locals(&result_tys);

                    instrumented_body.append(&mut save_stack_to_locals(&result_tmps));
                    instrumented_body.extend_from_slice(&[
                        location.0,
                        location.1,
                    ]);
                    instrumented_body.append(&mut restore_locals_with_i64_handling(&result_tmps, &result_tys));
                    instrumented_body.extend_from_slice(&[
                        polymorphic_hooks.get_call(&instr, result_tys),
                        instr,
                    ]);
                }
                (_, Call(target_func_idx)) => {
                    let arg_tys = module_info.functions[target_func_idx.0].type_.params.as_slice();
                    let result_tys = module_info.functions[target_func_idx.0].type_.results.as_slice();

                    type_stack.op(arg_tys, result_tys);

                    let arg_tmps = function.add_fresh_locals(arg_tys);
                    let result_tmps = function.add_fresh_locals(result_tys);

                    /* pre call hook */

                    instrumented_body.append(&mut save_stack_to_locals(&arg_tmps));
                    instrumented_body.extend_from_slice(&[
                        location.0.clone(),
                        location.1.clone(),
                        I32Const(target_func_idx.0 as i32),
                    ]);
                    instrumented_body.append(&mut restore_locals_with_i64_handling(&arg_tmps, &arg_tys));
                    instrumented_body.extend_from_slice(&[
                        polymorphic_hooks.get_call(&instr, arg_tys.to_vec()),
                        instr,
                    ]);

                    /* post call hook */

                    instrumented_body.append(&mut save_stack_to_locals(&result_tmps));
                    instrumented_body.extend_from_slice(&[
                        location.0,
                        location.1,
                    ]);
                    instrumented_body.append(&mut restore_locals_with_i64_handling(&result_tmps, &result_tys));
                    instrumented_body.push(Call(*call_result_hooks.get(result_tys).expect("no call_result hook for tys")));
                }
                (_, CallIndirect(func_ty, _ /* table idx == 0 in WASM version 1 */)) => {
                    let arg_tys = func_ty.params.as_slice();
                    let result_tys = func_ty.results.as_slice();

                    type_stack.op(arg_tys, result_tys);

                    let target_table_idx_tmp = function.add_fresh_local(I32);
                    let arg_tmps = function.add_fresh_locals(arg_tys);
                    let result_tmps = function.add_fresh_locals(result_tys);

                    /* pre call hook */

                    // TODO unify call args and target_table_idx_tmp
                    instrumented_body.push(SetLocal(target_table_idx_tmp));
                    instrumented_body.append(&mut save_stack_to_locals(&arg_tmps));
                    instrumented_body.extend_from_slice(&[
                        GetLocal(target_table_idx_tmp),
                        location.0.clone(),
                        location.1.clone(),
                        GetLocal(target_table_idx_tmp),
                    ]);
                    instrumented_body.append(&mut restore_locals_with_i64_handling(&arg_tmps, &arg_tys));
                    instrumented_body.extend_from_slice(&[
                        polymorphic_hooks.get_call(&instr, arg_tys.to_vec()),
                        instr,
                    ]);

                    /* post call hook */

                    instrumented_body.append(&mut save_stack_to_locals(&result_tmps));
                    instrumented_body.extend_from_slice(&[
                        location.0,
                        location.1,
                    ]);
                    instrumented_body.append(&mut restore_locals_with_i64_handling(&result_tmps, &result_tys));
                    instrumented_body.push(Call(*call_result_hooks.get(result_tys).expect("no call_result hook for tys")));
                }
                (Const(ty), instr) => {
                    type_stack.op(&[], &[ty]);

                    // TODO reorder hook and original instruction to make
                    // a) cheaper to construct
                    // b) easier to understand
                    // c) more regular between different hooks (i.e., hook always before instr or after)
                    instrumented_body.extend_from_slice(&[
                        location.0,
                        location.1,
                    ]);
                    instrumented_body.append(&mut convert_i64_instr(instr.clone(), ty));
                    instrumented_body.extend_from_slice(&[
                        monomorphic_hook_call(&instr),
                        instr,
                    ]);
                }
                // TODO unify Unary and Binary instrs
                (Unary { input_ty, result_ty }, instr) => {
                    type_stack.op(&[input_ty], &[result_ty]);

                    let input_tmp = function.add_fresh_local(input_ty);
                    let result_tmp = function.add_fresh_local(result_ty);

                    instrumented_body.extend_from_slice(&[
                        TeeLocal(input_tmp),
                        instr.clone(),
                        TeeLocal(result_tmp),
                        location.0,
                        location.1,
                    ]);
                    // restore saved input and result
                    instrumented_body.append(&mut restore_locals_with_i64_handling(&[input_tmp, result_tmp], &[input_ty, result_ty]));
                    instrumented_body.push(monomorphic_hook_call(&instr));
                }
                (Binary { first_ty, second_ty, result_ty }, instr) => {
                    type_stack.op(&[first_ty, second_ty], &[result_ty]);

                    let first_tmp = function.add_fresh_local(first_ty);
                    let second_tmp = function.add_fresh_local(second_ty);
                    let result_tmp = function.add_fresh_local(result_ty);

                    instrumented_body.append(&mut save_stack_to_locals(&[first_tmp, second_tmp]));
                    instrumented_body.extend_from_slice(&[
                        instr.clone(),
                        TeeLocal(result_tmp),
                        location.0,
                        location.1,
                    ]);
                    instrumented_body.append(&mut restore_locals_with_i64_handling(&[first_tmp, second_tmp, result_tmp], &[first_ty, second_ty, result_ty]));
                    instrumented_body.push(monomorphic_hook_call(&instr));
                }
                // TODO maybe unify Mem load and store?
                (MemoryLoad(ty, memarg), instr) => {
                    type_stack.op(&[I32], &[ty]);

                    let addr_tmp = function.add_fresh_local(I32);
                    let value_tmp = function.add_fresh_local(ty);

                    instrumented_body.extend_from_slice(&[
                        TeeLocal(addr_tmp),
                        instr.clone(),
                        TeeLocal(value_tmp),
                        location.0,
                        location.1,
                        I32Const(memarg.offset as i32),
                        I32Const(memarg.alignment as i32),
                    ]);
                    instrumented_body.append(&mut restore_locals_with_i64_handling(&[addr_tmp, value_tmp], &[I32, ty]));
                    instrumented_body.push(monomorphic_hook_call(&instr));
                }
                (MemoryStore(ty, memarg), instr) => {
                    type_stack.op(&[I32, ty], &[]);

                    let addr_tmp = function.add_fresh_local(I32);
                    let value_tmp = function.add_fresh_local(ty);

                    instrumented_body.append(&mut save_stack_to_locals(&[addr_tmp, value_tmp]));
                    instrumented_body.extend_from_slice(&[
                        instr.clone(),
                        location.0,
                        location.1,
                        I32Const(memarg.offset as i32),
                        I32Const(memarg.alignment as i32),
                    ]);
                    instrumented_body.append(&mut restore_locals_with_i64_handling(&[addr_tmp, value_tmp], &[I32, ty]));
                    instrumented_body.push(monomorphic_hook_call(&instr));
                }
                (_, Br(target_label)) => instrumented_body.extend_from_slice(&[
                    location.0,
                    location.1,
                    I32Const(target_label.0 as i32),
                    I32Const(label_to_instr_idx(&block_stack, target_label) as i32),
                    Call(br_hook),
                    instr
                ]),
                (_, BrIf(target_label)) => {
                    type_stack.op(&[I32], &[]);

                    let condition_tmp = function.add_fresh_local(I32);

                    instrumented_body.extend_from_slice(&[
                        TeeLocal(condition_tmp),
                        location.0,
                        location.1,
                        I32Const(target_label.0 as i32),
                        I32Const(label_to_instr_idx(&block_stack, target_label) as i32),
                        GetLocal(condition_tmp),
                        Call(br_if_hook),
                        instr
                    ]);
                }
                (_, BrTable(target_table, default_target)) => {
                    type_stack.op(&[I32], &[]);

                    module_info.br_tables.push(BrTableInfo::new(
                        target_table.into_iter().map(|label| LabelAndLocation::new(label.0)).collect(),
                        LabelAndLocation::new(default_target.0),
                    ));

                    let target_idx_tmp = function.add_fresh_local(I32);

                    instrumented_body.extend_from_slice(&[
                        TeeLocal(target_idx_tmp),
                        location.0,
                        location.1,
                        I32Const((module_info.br_tables.len() - 1) as i32),
                        GetLocal(target_idx_tmp),
                        Call(br_table_hook),
                        instr
                    ]);
                }
                _ => unreachable!("no hook for instruction {}", instr.to_instr_name()),
            }
        }

        // finally, move instrumented body inside function
        ::std::mem::replace(&mut function.code.as_mut().unwrap().body, instrumented_body);

        assert!(block_stack.is_empty(), "invalid begin/end nesting in function {}", fidx.0);
    }

    Some(js_codegen(module_info, &on_demand_hooks))
}

fn add_hook(module: &mut Module, name: impl Into<String>, arg_tys_: &[ValType]) -> Idx<Function> {
    // prepend two I32 for (function idx, instr idx)
    let mut arg_tys = vec![I32, I32];
    arg_tys.extend(arg_tys_.iter()
        // and expand i64 to a tuple of (i32, i32) since there is no JS interop for i64
        .flat_map(convert_i64_type));

    module.add_function_import(
        // hooks do not return anything
        FunctionType::new(arg_tys, vec![]),
        "hooks".into(),
        name.into())
}

// TODO put this in the MonomorphicHookMap.add() function instead
/// specialized version form of the above for monomorphic instructions
fn add_hook_from_instr(module: &mut Module, instr: &Instr, hooks: &mut Vec<String>) -> (Discriminant<Instr>, Idx<Function>) {
    hooks.push(instr.to_js_hook());
    (discriminant(instr), add_hook(module, instr.to_instr_name(), &match instr.group() {
        Const(ty) => vec![ty],
        Unary { input_ty, result_ty } => vec![input_ty, result_ty],
        Binary { first_ty, second_ty, result_ty } => vec![first_ty, second_ty, result_ty],
        // for address, offset and alignment
        MemoryLoad(ty, _) => vec![I32, I32, I32, ty],
        MemoryStore(ty, _) => vec![I32, I32, I32, ty],
        Other => unreachable!("function should be only called for \"grouped\" instructions"),
    }))
}

struct PolymorphicHookMap(HashMap<(Discriminant<Instr>, Vec<ValType>), Idx<Function>>);

impl PolymorphicHookMap {
    pub fn new() -> Self {
        PolymorphicHookMap(HashMap::new())
    }
    pub fn add(&mut self, module: &mut Module, instr: Instr, non_poly_args: &[ValType], tys: &[Vec<ValType>], hooks: &mut Vec<String>) {
        for tys in tys {
            hooks.push(instr.to_poly_js_hook(tys.as_slice()));
            let hook_name = append_mangled_tys(instr.to_instr_name(), tys.as_slice());
            let hook_idx = add_hook(module, hook_name, &[non_poly_args, tys.as_slice()].concat());
            self.0.insert(
                (discriminant(&instr), tys.clone()),
                hook_idx);
        }
    }
    pub fn get_call(&self, instr: &Instr, tys: Vec<ValType>) -> Instr {
        let error = format!("no hook was added for {} with types {:?}", instr.to_instr_name(), tys);
        Call(*self.0
            .get(&(discriminant(instr), tys))
            .expect(&error))
    }
}

/// helper function to save top locals.len() values into locals with the given index
/// types of locals must match stack, not enforced by this function!
fn save_stack_to_locals(locals: &[Idx<Local>]) -> Vec<Instr> {
    let mut instrs = Vec::new();
    // copy stack values into locals
    for &local in locals.iter().skip(1).rev() {
        instrs.push(SetLocal(local));
    }
    // optimization: for first local on the stack / last one saved use tee_local instead of set_local + get_local
    for &local in locals.iter().next() {
        instrs.push(TeeLocal(local));
    }
    // and restore (saving has removed them from the stack)
    for &local in locals.iter().skip(1) {
        instrs.push(GetLocal(local));
    }
    return instrs;
}

// TODO why not have a slice of tuples (Idx, ValType)?
fn restore_locals_with_i64_handling(locals: &[Idx<Local>], local_tys: &[ValType]) -> Vec<Instr> {
    assert_eq!(locals.len(), local_tys.len());

    let mut instrs = Vec::new();
    for (&local, &ty) in locals.iter().zip(local_tys.iter()) {
        instrs.append(&mut convert_i64_instr(GetLocal(local), ty));
    }
    return instrs;
}

// TODO move to own module, refactor (use InstructionLocation, not raw usize)
/// also keeps instruction index, needed later for End hooks
#[derive(Debug, PartialEq)]
enum Begin {
    // TODO include abstract block stack (i.e. Vec<ValType>) into this enum for
    // a) drop/select monomorphization
    // b) type checking
    // c) statically figuring out implicit drops during br/br_if/br_table
    // function begins correspond to no actual instruction, so no instruction index
    Function,
    Block(usize),
    Loop(usize),
    If(usize),
    Else(usize),
}

fn label_to_instr_idx(begin_stack: &[Begin], label: Idx<Label>) -> usize {
    let target_block = begin_stack.iter()
        .rev().nth(label.0)
        .expect(&format!("cannot resolve target for {:?}", label));
    match *target_block {
        Begin::Function => 0,
        Begin::Loop(begin_iidx) => begin_iidx,
        // FIXME if/else/block (forward jump, needs forward scanning for End)
        Begin::If(i) | Begin::Else(i) | Begin::Block(i) => i
    }
}