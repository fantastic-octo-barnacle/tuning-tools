//! Browse statics as a tree: roots are variables, children are struct members,
//! array elements and enum variants, expanded one level at a time.
//!
//! Nodes are addressed by [`NodeRef`] (symbol path plus steps), never by address,
//! so a reference stays meaningful after a rebuild moves things around.
//! Pointers are leaves: following them needs target memory.

use crate::dwarf_parser::DwarfDiagnostics;
use crate::elf::{ElfInfo, SymbolInfo};
use crate::type_table::{MemberDef, TypeDef, TypeId, TypeTable};
use crate::variable_type::VariableType;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Array elements returned per [`children`] call when no limit is given.
pub const DEFAULT_CHILD_LIMIT: usize = 256;

/// One step from a node to a child.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "camelCase")]
pub enum Step {
    Member(String),
    Index(u64),
    /// Payload of a Rust enum variant
    Variant(String),
    /// Tag member of a Rust enum
    Discriminant,
}

/// Stable reference to a node: full symbol path, then steps into its type.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NodeRef {
    pub symbol: String,
    pub steps: Vec<Step>,
}

impl NodeRef {
    pub fn root(symbol: impl Into<String>) -> Self {
        Self {
            symbol: symbol.into(),
            steps: Vec::new(),
        }
    }

    pub fn child(&self, step: Step) -> Self {
        let mut steps = self.steps.clone();
        steps.push(step);
        Self {
            symbol: self.symbol.clone(),
            steps,
        }
    }
}

/// `gimbal::GIMBAL.pitch.limit#Some.__0`, `SAMPLES[3]`, `cmd#<discriminant>`
impl fmt::Display for NodeRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.symbol)?;
        for step in &self.steps {
            match step {
                Step::Member(name) => write!(f, ".{name}")?,
                Step::Index(i) => write!(f, "[{i}]")?,
                Step::Variant(name) => write!(f, "#{name}")?,
                Step::Discriminant => f.write_str("#<discriminant>")?,
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum NodeKind {
    /// Integer, float or bool
    Scalar,
    /// C-like enum stored as an integer
    Enum,
    /// Rust enum with data (`DW_TAG_variant_part`)
    TaggedEnum,
    Struct,
    Union,
    Array,
    Pointer,
    Function,
    Other,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SymbolNode {
    #[serde(rename = "ref")]
    pub node: NodeRef,
    /// Display label: short symbol name, member name, `[i]` or variant name
    pub label: String,
    /// `node` rendered as a string
    pub path: String,
    pub address: u64,
    pub size: Option<u64>,
    pub type_name: String,
    pub kind: NodeKind,
    /// How to decode the bytes when the node is a single value
    pub scalar: Option<VariableType>,
    pub expandable: bool,
    /// Members / elements / variants, when known
    pub child_count: Option<u64>,
    pub readable: bool,
    /// Why the node cannot be read, when it cannot
    pub status: Option<String>,
    pub bit_offset: Option<u64>,
    pub bit_size: Option<u64>,
    /// For variant nodes: the tag value selecting this variant (`None` = default)
    pub discr_value: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RootNode {
    #[serde(flatten)]
    pub node: SymbolNode,
    /// Path split on `::`, respecting `<...>` (`<A as B>::info::INFO` has 3 segments)
    pub segments: Vec<String>,
    pub section: String,
    /// Not in an allocated, writable section: flash constants, vtables, defmt strings
    pub read_only: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Children {
    pub nodes: Vec<SymbolNode>,
    /// Total children; larger than `nodes.len()` when an array was truncated
    pub total: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ElfSummary {
    pub path: String,
    pub machine: String,
    pub is_64bit: bool,
    pub little_endian: bool,
    pub entry_point: u64,
    pub variables: usize,
    pub functions: usize,
    pub types: usize,
    pub diagnostics: DiagnosticsSummary,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsSummary {
    pub total_variables: usize,
    pub with_valid_address: usize,
    pub optimized_out: usize,
    pub local_variables: usize,
    pub extern_declarations: usize,
    pub compile_time_constants: usize,
    pub register_only: usize,
}

impl From<&DwarfDiagnostics> for DiagnosticsSummary {
    fn from(d: &DwarfDiagnostics) -> Self {
        Self {
            total_variables: d.total_variables,
            with_valid_address: d.with_valid_address,
            optimized_out: d.optimized_out,
            local_variables: d.local_variables,
            extern_declarations: d.extern_declarations,
            compile_time_constants: d.compile_time_constants,
            register_only: d.register_only,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TreeError {
    #[error("symbol not found: {0}")]
    SymbolNotFound(String),
    #[error("{symbol} has no type information")]
    Untyped { symbol: String },
    #[error("cannot step into {at}: {reason}")]
    BadStep { at: String, reason: String },
}

pub fn summary(elf: &ElfInfo) -> ElfSummary {
    ElfSummary {
        path: elf.path.clone(),
        machine: elf.machine.clone(),
        is_64bit: elf.is_64bit,
        little_endian: elf.is_little_endian,
        entry_point: elf.entry_point,
        variables: elf.variable_count(),
        functions: elf.function_count(),
        types: elf.type_table().len(),
        diagnostics: elf.get_diagnostics().into(),
    }
}

/// Every typed variable symbol, sorted by path.
pub fn roots(elf: &ElfInfo) -> Vec<RootNode> {
    let table = elf.type_table();
    let mut roots: Vec<RootNode> = elf
        .get_variables()
        .into_iter()
        .filter_map(|sym| {
            let type_id = sym.type_id?;
            let mut node = describe(
                table,
                NodeRef::root(&sym.demangled_name),
                sym.display_name.clone(),
                sym.address,
                type_id,
            );
            if node.size.is_none() && sym.size > 0 {
                node.size = Some(sym.size);
            }
            node.readable = sym.is_readable();
            node.status = sym.unreadable_reason().map(str::to_string);
            Some(RootNode {
                segments: split_path(&sym.demangled_name),
                read_only: !sym.writable,
                section: sym.section.clone(),
                node,
            })
        })
        .collect();
    roots.sort_by(|a, b| a.node.path.cmp(&b.node.path));
    roots.dedup_by(|a, b| a.node.path == b.node.path && a.node.address == b.node.address);
    roots
}

/// Describe the node `node` refers to.
pub fn node(elf: &ElfInfo, node: &NodeRef) -> Result<SymbolNode, TreeError> {
    let (sym, at) = resolve(elf, node)?;
    let mut out = describe(
        elf.type_table(),
        node.clone(),
        at.label,
        at.address,
        at.type_id,
    );
    out.bit_offset = at.bit_offset;
    out.bit_size = at.bit_size;
    out.discr_value = at.discr_value;
    if !sym.is_readable() {
        out.readable = false;
        out.status = sym.unreadable_reason().map(str::to_string);
    }
    Ok(out)
}

/// Children of `node`, at most `limit` array elements (default [`DEFAULT_CHILD_LIMIT`]).
pub fn children(
    elf: &ElfInfo,
    node: &NodeRef,
    limit: Option<usize>,
) -> Result<Children, TreeError> {
    let table = elf.type_table();
    let (sym, at) = resolve(elf, node)?;
    let limit = limit.unwrap_or(DEFAULT_CHILD_LIMIT) as u64;
    let readable = sym.is_readable();

    let member_node = |step: Step, m: &MemberDef, discr_value: Option<u64>| {
        let label = match &step {
            Step::Discriminant => "<discriminant>".to_string(),
            _ => m.name.clone(),
        };
        let mut n = describe(
            table,
            node.child(step),
            label,
            at.address + m.offset,
            m.type_id,
        );
        n.bit_offset = m.bit_offset;
        n.bit_size = m.bit_size;
        n.discr_value = discr_value;
        n.readable = readable;
        n
    };

    let nodes: Vec<SymbolNode> = match table.get(table.get_underlying(at.type_id)) {
        Some(TypeDef::Struct(s)) | Some(TypeDef::Union(s)) => match &s.variant_part {
            Some(part) => part
                .discriminant
                .iter()
                .map(|d| member_node(Step::Discriminant, d, None))
                .chain(part.variants.iter().map(|v| {
                    member_node(
                        Step::Variant(v.member.name.clone()),
                        &v.member,
                        v.discr_value,
                    )
                }))
                .collect(),
            None => s
                .members
                .iter()
                .map(|m| member_node(Step::Member(m.name.clone()), m, None))
                .collect(),
        },
        Some(TypeDef::Array {
            element,
            count: Some(count),
        }) => {
            let elem_size = table.type_size(*element).unwrap_or(0);
            let shown = (*count).min(limit);
            let nodes = (0..shown)
                .map(|i| {
                    let mut n = describe(
                        table,
                        node.child(Step::Index(i)),
                        format!("[{i}]"),
                        at.address + i * elem_size,
                        *element,
                    );
                    n.readable = readable;
                    n
                })
                .collect();
            return Ok(Children {
                nodes,
                total: *count,
            });
        }
        _ => Vec::new(),
    };
    let total = nodes.len() as u64;
    Ok(Children { nodes, total })
}

struct Resolved {
    label: String,
    address: u64,
    type_id: TypeId,
    bit_offset: Option<u64>,
    bit_size: Option<u64>,
    discr_value: Option<u64>,
}

fn resolve<'e>(elf: &'e ElfInfo, node: &NodeRef) -> Result<(&'e SymbolInfo, Resolved), TreeError> {
    let table = elf.type_table();
    let sym = elf
        .find_symbol(&node.symbol)
        .ok_or_else(|| TreeError::SymbolNotFound(node.symbol.clone()))?;
    let mut at = Resolved {
        label: sym.display_name.clone(),
        address: sym.address,
        type_id: sym.type_id.ok_or_else(|| TreeError::Untyped {
            symbol: node.symbol.clone(),
        })?,
        bit_offset: None,
        bit_size: None,
        discr_value: None,
    };

    let mut walked = NodeRef::root(&node.symbol);
    for step in &node.steps {
        let bad = |reason: &str| TreeError::BadStep {
            at: walked.to_string(),
            reason: reason.to_string(),
        };
        let def = table.get(table.get_underlying(at.type_id));
        let (member, discr_value) = match (step, def) {
            (Step::Member(name), Some(TypeDef::Struct(s) | TypeDef::Union(s))) => (
                s.members
                    .iter()
                    .find(|m| &m.name == name)
                    .ok_or_else(|| bad(&format!("no member `{name}`")))?,
                None,
            ),
            (Step::Variant(name), Some(TypeDef::Struct(s))) => {
                let v = s
                    .variant_part
                    .as_ref()
                    .and_then(|p| p.variants.iter().find(|v| &v.member.name == name))
                    .ok_or_else(|| bad(&format!("no variant `{name}`")))?;
                (&v.member, v.discr_value)
            }
            (Step::Discriminant, Some(TypeDef::Struct(s))) => (
                s.variant_part
                    .as_ref()
                    .and_then(|p| p.discriminant.as_ref())
                    .ok_or_else(|| bad("not a tagged enum"))?,
                None,
            ),
            (Step::Index(i), Some(TypeDef::Array { element, count })) => {
                if count.is_some_and(|c| *i >= c) {
                    return Err(bad(&format!("index {i} out of bounds")));
                }
                let elem_size = table.type_size(*element).unwrap_or(0);
                at = Resolved {
                    label: format!("[{i}]"),
                    address: at.address + i * elem_size,
                    type_id: *element,
                    bit_offset: None,
                    bit_size: None,
                    discr_value: None,
                };
                walked = walked.child(step.clone());
                continue;
            }
            _ => return Err(bad("step does not match the type")),
        };
        at = Resolved {
            label: match step {
                Step::Discriminant => "<discriminant>".to_string(),
                _ => member.name.clone(),
            },
            address: at.address + member.offset,
            type_id: member.type_id,
            bit_offset: member.bit_offset,
            bit_size: member.bit_size,
            discr_value,
        };
        walked = walked.child(step.clone());
    }
    Ok((sym, at))
}

fn describe(
    table: &TypeTable,
    node: NodeRef,
    label: String,
    address: u64,
    type_id: TypeId,
) -> SymbolNode {
    let underlying = table.get(table.get_underlying(type_id));
    let (kind, child_count) = match underlying {
        Some(TypeDef::Primitive(_)) => (NodeKind::Scalar, None),
        Some(TypeDef::Enum(_)) => (NodeKind::Enum, None),
        Some(TypeDef::Struct(s)) => match &s.variant_part {
            Some(p) => (
                NodeKind::TaggedEnum,
                Some((p.variants.len() + usize::from(p.discriminant.is_some())) as u64),
            ),
            None => (NodeKind::Struct, Some(s.members.len() as u64)),
        },
        Some(TypeDef::Union(s)) => (NodeKind::Union, Some(s.members.len() as u64)),
        Some(TypeDef::Array { count, .. }) => (NodeKind::Array, *count),
        Some(TypeDef::Pointer(_) | TypeDef::Reference(_)) => (NodeKind::Pointer, None),
        Some(TypeDef::Subroutine { .. }) => (NodeKind::Function, None),
        _ => (NodeKind::Other, None),
    };
    let scalar = matches!(kind, NodeKind::Scalar | NodeKind::Enum | NodeKind::Pointer)
        .then(|| table.to_variable_type(table.get_underlying(type_id)));
    SymbolNode {
        path: node.to_string(),
        node,
        label,
        address,
        size: table.type_size(type_id),
        type_name: table.type_name(type_id),
        kind,
        scalar,
        expandable: child_count.is_some_and(|c| c > 0),
        child_count,
        readable: true,
        status: None,
        bit_offset: None,
        bit_size: None,
        discr_value: None,
    }
}

/// Split a demangled path on `::` outside `<...>`, `(...)` and `[...]`.
pub fn split_path(path: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut depth = 0i32;
    let mut start = 0;
    let bytes = path.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'<' | b'(' | b'[' => depth += 1,
            b'>' | b')' | b']' => depth -= 1,
            b':' if depth == 0 && bytes.get(i + 1) == Some(&b':') => {
                segments.push(path[start..i].to_string());
                i += 2;
                start = i;
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    segments.push(path[start..].to_string());
    segments
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::elf::ElfParser;

    const RUST_V0_ELF: &[u8] = include_bytes!("../tests/fixtures/rust_v0.elf");
    const TEST_STRUCT_ELF: &[u8] = include_bytes!("../tests/fixtures/test_struct.elf");
    const GIMBAL: &str = "rust_fixture::control::GIMBAL";

    fn rust() -> ElfInfo {
        ElfParser::parse_bytes(RUST_V0_ELF, "rust_v0.elf").unwrap()
    }

    fn gimbal_value() -> NodeRef {
        NodeRef::root(GIMBAL)
            .child(Step::Member("__0".into()))
            .child(Step::Member("value".into()))
    }

    fn labels(c: &Children) -> Vec<&str> {
        c.nodes.iter().map(|n| n.label.as_str()).collect()
    }

    #[test]
    fn split_path_respects_generics_and_qualified_paths() {
        assert_eq!(split_path("a::b::C"), ["a", "b", "C"]);
        assert_eq!(split_path("C_COUNTER"), ["C_COUNTER"]);
        assert_eq!(
            split_path("<embassy_stm32::usart::UART5 as embassy_stm32::usart::SealedInstance>::state::STATE"),
            [
                "<embassy_stm32::usart::UART5 as embassy_stm32::usart::SealedInstance>",
                "state",
                "STATE"
            ]
        );
        assert_eq!(
            split_path("m::f::<u8, [u8; 2]>::X"),
            ["m", "f", "<u8, [u8; 2]>", "X"]
        );
    }

    #[test]
    fn roots_list_typed_statics_with_segments() {
        let elf = rust();
        let roots = roots(&elf);
        let samples = roots
            .iter()
            .find(|r| r.node.path == "rust_fixture::telemetry::SAMPLES")
            .unwrap();
        assert_eq!(samples.segments, ["rust_fixture", "telemetry", "SAMPLES"]);
        assert_eq!(samples.node.label, "SAMPLES");
        assert_eq!(samples.node.kind, NodeKind::Array);
        assert_eq!(samples.node.child_count, Some(8));
        assert!(!samples.read_only);

        let reset = roots
            .iter()
            .find(|r| r.node.label == "RESET_VECTOR")
            .unwrap();
        assert!(reset.read_only, "section {}", reset.section);
        assert_eq!(reset.node.kind, NodeKind::Pointer);
    }

    #[test]
    fn walk_struct_members_down_to_scalars() {
        let elf = rust();
        let sym_addr = elf.find_symbol(GIMBAL).unwrap().address;
        let value = children(&elf, &NodeRef::root(GIMBAL), None).unwrap();
        assert_eq!(labels(&value), ["__0"]);

        let fields = children(&elf, &gimbal_value(), None).unwrap();
        let mut sorted = labels(&fields);
        sorted.sort();
        assert_eq!(
            sorted,
            [
                "command",
                "history",
                "last_fault",
                "mode",
                "offset",
                "pitch",
                "yaw"
            ]
        );

        let kp_ref = gimbal_value()
            .child(Step::Member("yaw".into()))
            .child(Step::Member("kp".into()));
        let kp = node(&elf, &kp_ref).unwrap();
        assert_eq!(kp.path, "rust_fixture::control::GIMBAL.__0.value.yaw.kp");
        assert_eq!(kp.kind, NodeKind::Scalar);
        assert_eq!(kp.scalar, Some(VariableType::F32));
        assert_eq!(kp.address, sym_addr + 28);
        assert!(!kp.expandable);

        let mode = node(&elf, &gimbal_value().child(Step::Member("mode".into()))).unwrap();
        assert_eq!(mode.kind, NodeKind::Enum);
        assert_eq!(mode.scalar, Some(VariableType::U8));
    }

    #[test]
    fn arrays_index_and_truncate() {
        let elf = rust();
        let samples = NodeRef::root("rust_fixture::telemetry::SAMPLES");
        let base = elf.find_symbol(&samples.symbol).unwrap().address;

        let all = children(&elf, &samples, None).unwrap();
        assert_eq!(all.total, 8);
        assert_eq!(all.nodes[3].path, "rust_fixture::telemetry::SAMPLES[3]");
        assert_eq!(all.nodes[3].address, base + 6);
        assert_eq!(all.nodes[3].type_name, "u16");

        let some = children(&elf, &samples, Some(2)).unwrap();
        assert_eq!((some.nodes.len(), some.total), (2, 8));

        let out = node(&elf, &samples.child(Step::Index(8)));
        assert!(matches!(out, Err(TreeError::BadStep { .. })), "{out:?}");
    }

    #[test]
    fn tagged_enum_children_are_discriminant_then_variants() {
        let elf = rust();
        let command = gimbal_value().child(Step::Member("command".into()));
        let c = node(&elf, &command).unwrap();
        assert_eq!(c.kind, NodeKind::TaggedEnum);
        assert_eq!(c.child_count, Some(4));

        let kids = children(&elf, &command, None).unwrap();
        assert_eq!(
            labels(&kids),
            ["<discriminant>", "Stop", "Velocity", "Position"]
        );
        assert_eq!(kids.nodes[0].scalar, Some(VariableType::U32));
        assert_eq!(kids.nodes[0].path, format!("{command}#<discriminant>"));
        assert_eq!(kids.nodes[3].discr_value, Some(2));

        let target = command
            .child(Step::Variant("Position".into()))
            .child(Step::Member("target".into()));
        let t = node(&elf, &target).unwrap();
        assert_eq!(t.path, format!("{command}#Position.target"));
        assert_eq!(t.address, c.address + 4);
        assert_eq!(t.scalar, Some(VariableType::F32));
    }

    #[test]
    fn bad_references_are_errors() {
        let elf = rust();
        assert_eq!(
            node(&elf, &NodeRef::root("nope")).unwrap_err(),
            TreeError::SymbolNotFound("nope".into())
        );
        let err = node(&elf, &gimbal_value().child(Step::Member("roll".into()))).unwrap_err();
        assert_eq!(
            err.to_string(),
            "cannot step into rust_fixture::control::GIMBAL.__0.value: no member `roll`"
        );
    }

    #[test]
    fn c_structs_browse_the_same_way() {
        let elf = ElfParser::parse_bytes(TEST_STRUCT_ELF, "test_struct.elf").unwrap();
        let cfg = NodeRef::root("device_config");
        let kids = children(&elf, &cfg, None).unwrap();
        assert_eq!(labels(&kids), ["id", "sensor", "enabled"]);
        let value = node(
            &elf,
            &cfg.child(Step::Member("sensor".into()))
                .child(Step::Member("value".into())),
        )
        .unwrap();
        assert_eq!(value.scalar, Some(VariableType::F32));
        assert_eq!(
            value.address,
            elf.find_symbol("device_config").unwrap().address + 12
        );
    }

    #[test]
    fn node_ref_serializes_as_tagged_steps() {
        let r = NodeRef::root("X")
            .child(Step::Member("a".into()))
            .child(Step::Index(2))
            .child(Step::Discriminant);
        let json = serde_json::to_string(&r).unwrap();
        assert_eq!(
            json,
            r#"{"symbol":"X","steps":[{"kind":"member","value":"a"},{"kind":"index","value":2},{"kind":"discriminant"}]}"#
        );
        assert_eq!(serde_json::from_str::<NodeRef>(&json).unwrap(), r);
    }
}
