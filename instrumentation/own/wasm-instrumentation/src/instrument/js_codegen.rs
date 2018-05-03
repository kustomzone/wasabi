use ast::highlevel::Instr::{self, *};
use ast::ValType::{self, *};
use serde_json;
use super::static_info::ModuleInfo;

pub fn js_codegen(module_info: ModuleInfo, on_demand_hooks: &[String]) -> String {
    format!(r#"/*
 * Auto-generated from WASM module to-analyze.
 * DO NOT EDIT.
 */

Wasabi.module.info = {};

Wasabi.module.lowlevelHooks = {{
{}{}
}};
"#,
            serde_json::to_string_pretty(&module_info).unwrap(),
            r#"
    start: function(func, instr) {
        start({func, instr});
    },

    nop: function (func, instr) {
        nop({func, instr});
    },
    unreachable: function (func, instr) {
        unreachable({func, instr});
    },

    memory_size: function (func, instr, currentSizePages) {
        memory_size({func, instr}, currentSizePages);
    },
    memory_grow: function (func, instr, byPages, previousSizePages) {
        memory_grow({func, instr}, byPages, previousSizePages);
    },

    // begin/ends
    begin_function: function (func, instr) {
        begin({func, instr}, "function");
    },
    end_function: function (func, instr) {
        end({func, instr}, "function", {func, instr: -1});
    },
    begin_block: function (func, instr) {
        begin({func, instr}, "block");
    },
    end_block: function (func, instr, begin_instr) {
        end({func, instr}, "block", {func, instr: begin_instr});
    },
    begin_loop: function (func, instr) {
        begin({func, instr}, "loop");
    },
    end_loop: function (func, instr, begin_instr) {
        end({func, instr}, "loop", {func, instr: begin_instr});
    },
    begin_if: function (func, instr) {
        begin({func, instr}, "if");
    },
    end_if: function (func, instr, if_instr) {
        end({func, instr}, "if", {func, instr: if_instr});
    },
    begin_else: function (func, instr, if_instr) {
        begin({func, instr}, "else", {func, instr: if_instr});
    },
    end_else: function (func, instr, if_instr, else_instr) {
        end({func, instr}, "else", {func, instr: if_instr}, {func, instr: else_instr});
    },

    // branches/if condition
    if_: function (func, instr, condition) {
        if_({func, instr}, condition === 1);
    },
    br: function (func, instr, target_label, target_instr) {
        br({func, instr}, {label: target_label, location: {func, instr: target_instr}});
    },
    br_if: function (func, instr, target_label, target_instr, condition) {
        br_if({func, instr}, {label: target_label, location: {func, instr: target_instr}}, condition === 1);
    },
    br_table: function (func, instr, br_table_info_idx, table_idx) {
        br_table({
            func,
            instr
        }, Wasabi.module.info.brTables[br_table_info_idx].table, Wasabi.module.info.brTables[br_table_info_idx].default, table_idx);
    },

    // generated:
    "#,
            on_demand_hooks.iter().flat_map(|s| s.split("\n")).collect::<Vec<&str>>().join("\n    ")
    )
}

/// "generate" quick and dirty the low-level JavaScript hook function from an instruction
impl Instr {
    pub fn to_js_hook(&self) -> String {
        let instr_name = self.to_name();
        match (self, self.to_type()) {
            (Const(val), _) => format!(
                "\"{}\": function (func, instr, {}) {{
    const_({{func, instr}}, {});
}},",
                instr_name,
                arg("v", val.to_type()), long("v", val.to_type())
            ),
            (Numeric(_), Some(ref ty)) if ty.inputs.len() == 1 => format!(
                "\"{}\": function (func, instr, {}, {}) {{
    unary({{func, instr}}, \"{}\", {}, {});
}},",
                instr_name,
                arg("input", ty.inputs[0]), arg("result", ty.results[0]),
                instr_name,
                long("input", ty.inputs[0]), long("result", ty.results[0])),
            (Numeric(_), Some(ref ty)) if ty.inputs.len() == 2 => format!(
                "\"{}\": function (func, instr, {}, {}, {}) {{
    binary({{func, instr}}, \"{}\", {}, {}, {});
}},",
                instr_name,
                arg("first", ty.inputs[0]), arg("second", ty.inputs[1]), arg("result", ty.results[0]),
                instr_name,
                long("first", ty.inputs[0]), long("second", ty.inputs[1]), long("result", ty.results[0])),
            (Load(_, _), Some(ty)) => format!(
                "\"{}\": function (func, instr, offset, align, addr, {}) {{
    load({{func, instr}}, \"{}\", {{addr, offset, align}}, {});
}},",
                instr_name,
                arg("v", ty.results[0]),
                instr_name,
                long("v", ty.results[0])),
            (Store(_, _), Some(ty)) => format!(
                "\"{}\": function (func, instr, offset, align, addr, {}) {{
    store({{func, instr}}, \"{}\", {{addr, offset, align}}, {});
}},",
                instr_name,
                arg("v", ty.inputs[0]),
                instr_name,
                long("v", ty.inputs[0])),
            _ => unimplemented!("cannot generate JS hook code for instruction {}", instr_name)
        }
    }

    pub fn to_poly_js_hook(&self, tys: &[ValType]) -> String {
        let hook_name = append_mangled_tys(self.to_name().to_string(), tys);
        match *self {
            Return => {
                let return_hook = format!("{}: function(func, instr{}) {{
    return_({{func, instr}}, [{}]);
}},",
                                          hook_name,
                                          tys.iter().enumerate().map(|(i, ty)| format!(", {}", arg(&("result".to_string() + &i.to_string()), *ty))).collect::<String>(),
                                          tys.iter().enumerate().map(|(i, ty)| long(&("result".to_string() + &i.to_string()), *ty)).collect::<Vec<String>>().join(", "),
                );
                return_hook.clone()
                    + "\n"
                    + &return_hook
                    // quick&dirty hack: call_post hook has same signature as return, so generate it just by textual replacement
                    .replace("return", "call_post")
                    // remove trailing underscore because of textual replacement
                    .replace("call_post_({", "call_post({")
            }
            Call(_) => format!("{}: function(func, instr, targetFunc{}) {{
    call_pre({{func, instr}}, targetFunc, false, [{}]);
}},",
                               hook_name,
                               tys.iter().enumerate().map(|(i, ty)| format!(", {}", arg(&("arg".to_string() + &i.to_string()), *ty))).collect::<String>(),
                               tys.iter().enumerate().map(|(i, ty)| long(&("arg".to_string() + &i.to_string()), *ty)).collect::<Vec<String>>().join(", "),
            ),
            CallIndirect(_, _) => format!("{}: function(func, instr, targetTableIdx{}) {{
    call_pre({{func, instr}}, Wasabi.resolveTableIdx(targetTableIdx), true, [{}]);
}},",
                                          hook_name,
                                          tys.iter().enumerate().map(|(i, ty)| format!(", {}", arg(&("arg".to_string() + &i.to_string()), *ty))).collect::<String>(),
                                          tys.iter().enumerate().map(|(i, ty)| long(&("arg".to_string() + &i.to_string()), *ty)).collect::<Vec<String>>().join(", "),
            ),
            Drop => format!("{}: function(func, instr, {}) {{
    drop({{func, instr}}, {});
}},",
                            hook_name,
                            arg("v", tys[0]),
                            long("v", tys[0])
            ),
            Select => format!("{}: function(func, instr, condition, {}, {}) {{
    select({{func, instr}}, condition === 1, {}, {});
}},",
                              hook_name,
                              arg("first", tys[0]), arg("second", tys[1]),
                              long("first", tys[0]), long("second", tys[1]),
            ),
            Local(_, _) => format!("{}: function(func, instr, index, {}) {{
    local({{func, instr}}, \"{}\", index, {});
}},",
                                   hook_name,
                                   arg("v", tys[0]),
                                   self.to_name(),
                                   long("v", tys[0])
            ),
            Global(_, _) => format!("{}: function(func, instr, index, {}) {{
    global({{func, instr}}, \"{}\", index, {});
}},",
                                    hook_name,
                                    arg("v", tys[0]),
                                    self.to_name(),
                                    long("v", tys[0])
            ),
            _ => unimplemented!("cannot generate JS hook code for instruction {}", self.to_name())
        }
    }
}


/* quick & dirty helpers */

/// e.g. "call" + [I32, F32] -> "call_i32_f32"
pub fn append_mangled_tys(prefix: String, tys: &[ValType]) -> String {
    prefix + "_" + &tys.iter().map(|ty| ty.to_string()).collect::<Vec<_>>().join("_")
}

fn arg(name: &str, ty: ValType) -> String {
    match ty {
        I64 => name.to_string() + "_low, " + name + "_high",
        _ => name.to_string()
    }
}

fn long(name: &str, ty: ValType) -> String {
    match ty {
        I64 => format!("new Long({})", arg(name, ty)),
        _ => name.to_string()
    }
}
