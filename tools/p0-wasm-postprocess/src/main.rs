use std::{env, fs};

use wasm_encoder::{
    EntityType, ExportKind, ExportSection, ImportSection, Module, RawSection, Section,
};
use wasmparser::{ExternalKind, Parser, Payload, TypeRef};
use wasmi::{CompilationMode, Config, Engine, Module as WasmiModule, StackLimits};

const MAX_MEMORY_PAGES: u64 = 16;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = env::args_os().collect();
    if args.len() != 3 {
        return Err("usage: p0-wasm-postprocess INPUT OUTPUT".into());
    }

    let input = fs::read(&args[1])?;
    let output = process(&input)?;
    fs::write(&args[2], &output)?;
    println!("deployable_wasm_bytes={}", output.len());
    Ok(())
}

fn process(input: &[u8]) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut output = Vec::new();
    let mut memory_imports = 0usize;
    let mut call_export = false;
    let mut deploy_export = false;

    for payload in Parser::new(0).parse_all(input) {
        let payload = payload?;
        match payload {
            Payload::Version { encoding, .. } => {
                if encoding != wasmparser::Encoding::Module {
                    return Err("component modules are not supported".into());
                }
                output.extend_from_slice(&Module::HEADER);
            }
            Payload::End(_) => break,
            Payload::CustomSection(_) => {}
            Payload::ImportSection(reader) => {
                let mut imports = ImportSection::new();
                for entry in reader {
                    let entry = entry?;
                    match entry.ty {
                        TypeRef::Func(_) if entry.module.starts_with("seal") => {}
                        TypeRef::Func(_) => {
                            return Err(format!("unsupported contract import {}.{}", entry.module, entry.name).into());
                        }
                        TypeRef::Memory(memory) => {
                            if entry.module != "env" || entry.name != "memory" {
                                return Err("contract memory must be imported as env.memory".into());
                            }
                            memory_imports += 1;
                            if memory.initial > MAX_MEMORY_PAGES {
                                return Err("contract memory minimum exceeds 16 pages".into());
                            }
                            if memory.maximum.is_some_and(|maximum| maximum > MAX_MEMORY_PAGES) {
                                return Err("contract memory maximum exceeds 16 pages".into());
                            }
                        }
                        _ => return Err("contract tables and globals cannot be imported".into()),
                    }
                    let mut entity = EntityType::try_from(entry.ty)?;
                    if let EntityType::Memory(memory) = &mut entity {
                        memory.maximum = Some(MAX_MEMORY_PAGES);
                    }
                    imports.import(entry.module, entry.name, entity);
                }
                imports.append_to(&mut output);
            }
            Payload::ExportSection(reader) => {
                let mut exports = ExportSection::new();
                for entry in reader {
                    let entry = entry?;
                    if entry.kind != ExternalKind::Func {
                        continue;
                    }
                    match entry.name {
                        "call" => call_export = true,
                        "deploy" => deploy_export = true,
                        _ => continue,
                    }
                    exports.export(entry.name, ExportKind::Func, entry.index);
                }
                exports.append_to(&mut output);
            }
            Payload::MemorySection(_) => {
                return Err("contract linear memory must be imported".into());
            }
            _ => {
                if let Some((id, range)) = payload.as_section() {
                    RawSection {
                        id,
                        data: &input[range],
                    }
                    .append_to(&mut output);
                }
            }
        }
    }

    if memory_imports != 1 {
        return Err("contract must import exactly one env.memory".into());
    }
    if !call_export || !deploy_export {
        return Err("contract must export call and deploy".into());
    }

    validate_runtime_wasm(&output)?;
    Ok(output)
}

fn validate_runtime_wasm(wasm: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    let mut config = Config::default();
    config
        .wasm_multi_value(false)
        .wasm_mutable_global(false)
        .wasm_sign_extension(true)
        .wasm_bulk_memory(false)
        .wasm_reference_types(false)
        .wasm_tail_call(false)
        .wasm_extended_const(false)
        .wasm_saturating_float_to_int(false)
        .floats(false)
        .compilation_mode(CompilationMode::Eager)
        .consume_fuel(true);
    config.set_stack_limits(
        StackLimits::new(1, 1, 0).map_err(|error| format!("{error:?}"))?,
    );
    let engine = Engine::new(&config);
    WasmiModule::new(&engine, wasm).map_err(|error| format!("{error:?}"))?;
    Ok(())
}
