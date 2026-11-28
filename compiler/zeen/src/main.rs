use clap::{CommandFactory, Parser};
use rustc_hash::FxHashSet as HashSet;
use std::{cell::RefCell, env, io::Write, path::Path, process::exit, rc::Rc, sync::Arc};
use zeen_driver::{
    CORE_FILES, CompilationContext, CompilationMode, CompilationOutput, MietteDriver, PathsConfig,
};

mod cli;
mod targets;

fn warning_module_disables(
    source: Option<&miette::NamedSource<Arc<String>>>,
    packages: &[std::path::PathBuf],
) -> bool {
    source.is_some_and(|source| zeen_driver::source_disables_warnings(source, packages))
}

fn with_default_extension(path: &Path, ext: &str) -> std::path::PathBuf {
    if path
        .extension()
        .is_some_and(|existing| existing.to_string_lossy().eq_ignore_ascii_case(ext))
    {
        return path.to_path_buf();
    }

    let mut name = path.as_os_str().to_os_string();
    name.push(".");
    name.push(ext);
    std::path::PathBuf::from(name)
}

const COMPILER_THREAD_STACK: usize = 256 * 1024 * 1024;

struct StageTimer {
    enabled: bool,
    last: std::time::Instant,
}

impl StageTimer {
    fn new(enabled: bool) -> Self {
        Self {
            enabled,
            last: std::time::Instant::now(),
        }
    }

    fn mark(&mut self, label: &str) {
        if !self.enabled {
            return;
        }

        let now = std::time::Instant::now();

        cli::println_info(
            "Timing",
            format!(
                "{label:<12} {:>9.3} ms",
                (now - self.last).as_secs_f64() * 1000.0
            ),
        );

        self.last = now;
    }

    fn mark_elapsed(&mut self, label: &str, elapsed: std::time::Duration) {
        if !self.enabled {
            return;
        }

        cli::println_info(
            "Timing",
            format!("{label:<12} {:>9.3} ms", elapsed.as_secs_f64() * 1000.0),
        );

        self.last = std::time::Instant::now();
    }
}

fn run_codegen_unit<T>(
    program: &zeen_mir::MirProgram,
    typecheck: &zeen_typecheck::result::TypeCheckResult,
    resolution: &zeen_resolve::ResolutionResult,
    names: &zeen_codegen_llvm::CodeGenNames,
    options: zeen_codegen_llvm::CodegenOptions,
    emit: impl FnOnce(&zeen_codegen_llvm::CodeGen<'_, '_>) -> Result<T, zeen_codegen_llvm::CodegenError>,
) -> Result<T, zeen_codegen_llvm::CodegenError> {
    let context = inkwell::context::Context::create();
    let mut codegen = zeen_codegen_llvm::CodeGen::new(
        &context,
        program,
        typecheck,
        resolution,
        names.clone(),
        options,
    )?;
    codegen.generate()?;

    if cfg!(not(target_os = "windows")) {
        codegen.verify()?;
    }

    emit(&codegen)
}

fn parse_pkg_flags(flags: &[String]) -> Vec<(String, std::path::PathBuf)> {
    let mut out = Vec::new();
    for flag in flags {
        match flag.split_once('=') {
            Some((name, path))
                if !name.is_empty() && !name.contains(['.', '/', '\\']) && !path.is_empty() =>
            {
                let path = std::path::PathBuf::from(path);
                if !path.is_dir() {
                    cli::println_error(format!(
                        "--pkg `{flag}`: `{}` is not a directory",
                        path.display()
                    ));
                    exit(1);
                }
                out.push((name.to_string(), path));
            }
            _ => {
                cli::println_error(format!("--pkg `{flag}`: expected NAME=PATH"));
                exit(1);
            }
        };
    }
    out
}

fn resolve_std_root(explicit: Option<&Path>) -> Result<Option<std::path::PathBuf>, String> {
    const REPO: &str = "https://github.com/zeen-lang/zeen";

    fn std_src(root: &Path) -> Option<std::path::PathBuf> {
        let src = root.join("src");

        src.is_dir().then_some(src)
    }

    if let Some(path) = explicit {
        return match std_src(path) {
            Some(src) => Ok(Some(src)),
            None => Err(format!(
                "std library directory not found at `{}` (set by --std); the std project root must contain a `src` directory, see {REPO}",
                path.display()
            )),
        };
    }

    if let Some(env_path) = env::var_os("ZEEN_STD") {
        let path = std::path::PathBuf::from(env_path);

        return match std_src(&path) {
            Some(src) => Ok(Some(src)),
            None => Err(format!(
                "`ZEEN_STD` points to `{}` which has no `src` directory; see {REPO}",
                path.display()
            )),
        };
    }

    let home = env::var_os("HOME").or_else(|| env::var_os("USERPROFILE"));
    if let Some(path) = home
        .map(|home| Path::new(&home).join(".zeen").join("std"))
        .and_then(|path| std_src(&path))
    {
        return Ok(Some(path));
    }

    let next_to_exe = env::current_exe()
        .ok()
        .and_then(|exe| {
            exe.parent()?
                .parent()
                .map(|prefix| prefix.join("share").join("zeen").join("std"))
        })
        .and_then(|path| std_src(&path));

    Ok(next_to_exe)
}

fn main() {
    let args = cli::Args::try_parse().unwrap_or_else(|err| {
        let mut command = cli::Args::command();

        let authors_env = env!("CARGO_PKG_AUTHORS");
        let authors_fmt = if authors_env.contains(":") {
            format!("\n| {}", authors_env.replace(":", "\n| "))
        } else {
            authors_env.to_owned()
        };

        cli::println_primary("💤 Zeen Programming Language");
        cli::println_basic(format!("| - version: {}", env!("CARGO_PKG_VERSION")));
        cli::println_basic(format!("| - authors: {}", authors_fmt));

        match err.kind() {
            clap::error::ErrorKind::DisplayVersion => {
                exit(0);
            }

            _ => {
                cli::println_basic("");
                cli::println_primary("🍀 Options:");

                command.print_help().unwrap();

                cli::println_basic("");
                cli::println_primary("🎓 Examples of usage:");
                cli::println_basic("  zeen example.zn output");
                cli::println_basic("  zeen example.zn output -m Release");
                cli::println_basic("  zeen example.zn output --emit IR");
                cli::println_basic("  zeen example.zn output --target x86_64-pc-windows-gnu");
                cli::println_basic("  zeen --targets-list");

                if err.kind() == clap::error::ErrorKind::DisplayHelp {
                    exit(0);
                }

                exit(1);
            }
        }
    });

    // `--no-color`
    if args.no_color {
        colored::control::set_override(false);
    }

    if let Some(triple) = &args.target
        && !targets::is_supported(triple)
    {
        cli::println_error(format!("unsupported target triple `{triple}`"));
        cli::println_basic("\nSupported targets:");
        for supported in targets::SUPPORTED_TARGETS {
            cli::println_basic(format!("  {supported}"));
        }
        exit(1);
    }

    if args.targets_list {
        cli::println_primary(format!(
            "Supported targets ({}):",
            targets::SUPPORTED_TARGETS.len()
        ));

        for triple in targets::SUPPORTED_TARGETS {
            cli::println_basic(format!("  {triple}"));
        }

        exit(0);
    }

    // The pipeline runs on a dedicated thread with a big stack: recursive
    // passes need more than the 1 MB default main thread on Windows, while
    // Linux gives 8 MB.
    let result = std::thread::Builder::new()
        .name("zeen-compile".into())
        .stack_size(COMPILER_THREAD_STACK)
        .spawn(|| compile(args))
        .and_then(|child| child.join().map_err(|_| std::io::Error::other("ICE")));

    if result.is_err() {
        cli::println_error(
            "internal compiler error (ICE). This is a compiler bug; see https://github.com/zeen-lang/zeen",
        );
        exit(1);
    }
}

fn compile(args: cli::Args) {
    let compilation_start = std::time::Instant::now();
    let mut timer = StageTimer::new(args.timings);
    let target_triple = args.target.clone().unwrap_or_else(targets::host_target);

    let path = args
        .path
        .expect("path is required unless `--targets-list` is given");
    let output = match args.output.clone() {
        Some(output) => output,
        None if args.check => std::path::PathBuf::new(),
        None => {
            cli::println_error("output is required unless `--targets-list` is given");
            exit(1)
        }
    };

    let filename = path
        .file_name()
        .unwrap_or_else(|| {
            cli::println_error("Unable to get source filename");
            exit(1);
        })
        .to_str()
        .unwrap_or_else(|| {
            cli::println_error("Unable to get source filename");
            exit(1);
        });

    // cli::println_info(
    //     "Reading",
    //     format!(
    //         "`{}` ({})",
    //         filename,
    //         std::fs::canonicalize(&path)
    //             .unwrap_or_else(|_| {
    //                 cli::println_error(format!("File `{}` doesn't exist", filename));
    //                 exit(1);
    //             })
    //             .display()
    //     ),
    // );

    let rodeo = Rc::new(RefCell::new(lasso::Rodeo::default()));
    let bump = bumpalo::Bump::default();
    let driver = MietteDriver::new().use_color(!args.no_color);

    let content = {
        let src = std::fs::read_to_string(&path).unwrap_or_else(|err| {
            cli::println_error(format!(
                "Unable to read source ({}) file: {}",
                path.display(),
                err
            ));
            exit(1);
        });

        Arc::new(src)
    };

    timer.mark("read");

    let filename = Rc::new(filename.to_string());
    let project_root = std::fs::canonicalize(&path)
        .expect("already verified earlier")
        .parent()
        .expect("must work")
        .into();

    let std_root = match resolve_std_root(args.std.as_deref()) {
        Ok(path) => path,
        Err(message) => {
            cli::println_error(message);
            exit(1);
        }
    };

    let mut context = CompilationContext {
        paths: PathsConfig {
            project_root,
            std_root,
            linked: HashSet::default(),
            packages: parse_pkg_flags(&args.pkg),
        },
        core_files: CORE_FILES.iter().map(|file| file.to_basic()).collect(),
        mode: args.mode,
        output: args.emit,
        target: Some(target_triple.clone()),
        warnings: Vec::new(),
    };

    let mut quiet_dirs: Vec<std::path::PathBuf> = context
        .paths
        .packages
        .iter()
        .map(|(_, dir)| std::fs::canonicalize(dir).unwrap_or_else(|_| dir.clone()))
        .collect();

    if let Some(std_root) = &context.paths.std_root {
        quiet_dirs.push(std::fs::canonicalize(std_root).unwrap_or_else(|_| std_root.clone()));
    }

    cli::println_info(
        "Building",
        format!(
            "from `{filename}` entry (root: `{}`)",
            context.paths.project_root.display()
        ),
    );

    cli::println_info("Target", format!("triple `{target_triple}`"));

    let mut tokens = zeen_lexer::tokenize(&content);
    let mut parser = zeen_parser::Parser::new(
        Rc::clone(&filename),
        Arc::clone(&content),
        &mut tokens,
        &bump,
        Rc::clone(&rodeo),
    );

    // cli::println_info("Parsing", "abstract syntax tree");

    let program = parser.parse_program().unwrap_or_else(|errors| {
        for err in errors {
            let report_string = driver.report(err).unwrap_or_default();
            eprintln!("{}", report_string);
        }

        cli::println_error(format!("Compiler returned {} error(s)", errors.len()));

        exit(1);
    });

    timer.mark("parse");

    // cli::println_info("Preprocessing", "conditional declarations for target");

    let target = zeen_driver::Target::parse(&target_triple);
    let program = zeen_preprocessor::resolve(program, &bump, &rodeo, &target, context.mode);

    timer.mark("preprocess");

    // cli::println_info(
    //     "Resolving",
    //     format!("program ({} declarations)", program.len()),
    // );

    let (resolved_program, resolution_result) = zeen_resolve::resolve(
        Rc::clone(&filename),
        Arc::clone(&content),
        Path::new(&path),
        program,
        &bump,
        Rc::clone(&rodeo),
        &mut context,
    )
    .unwrap_or_else(|errors| {
        for err in &errors {
            let report_string = driver.report(err).unwrap_or_default();
            eprintln!("{}", report_string);
        }

        cli::println_error(format!("Compiler returned {} errors", errors.len()));

        exit(1);
    });

    timer.mark("resolve");

    let mut hir_lowering = zeen_hir::HirLowering::new(&resolution_result, Rc::clone(&rodeo));
    let hir_module = hir_lowering.lower_module(resolved_program);

    drop(bump);

    timer.mark("hir");

    if !args.no_warns {
        for warning in &context.warnings {
            cli::println_warn(warning);
        }
    }

    // cli::println_info(
    //     "Checking",
    //     format!(
    //         "resolved program ({} definitions)",
    //         resolution_result.defs.len()
    //     ),
    // );

    let mut typechecker =
        zeen_typecheck::TypeChecker::new(&resolution_result, &context, Rc::clone(&rodeo));
    typechecker.check_module(&hir_module);

    let mut typechecker_result = typechecker.finish().unwrap_or_else(|errors| {
        for err in &errors {
            let report_string = driver.report(err).unwrap_or_default();
            eprintln!("{}", report_string);
        }

        cli::println_error(format!("Compiler returned {} error(s)", errors.len()));

        exit(1);
    });

    timer.mark("typecheck");

    let mut lowered_mir = zeen_mir::lowering::lower_program(
        Rc::clone(&rodeo),
        &mut typechecker_result,
        &resolution_result,
        &hir_module,
        context.mode,
    )
    .unwrap_or_else(|errors| {
        for err in &errors {
            let report_string = driver.report(err).unwrap_or_default();
            eprintln!("{}", report_string);
        }

        cli::println_error(format!("Compiler returned {} error(s)", errors.len()));

        exit(1);
    });

    timer.mark("mir");

    let flow_result = zeen_flow::run_dataflow(
        &mut lowered_mir.program,
        &mut typechecker_result,
        &resolution_result,
        Rc::clone(&rodeo),
    );

    timer.mark("flow");

    match flow_result {
        Ok(result) => {
            if !args.no_warns {
                let mut warnings = Vec::new();

                for warning in lowered_mir.warnings.iter() {
                    if warning_module_disables(warning.source_code(), &quiet_dirs) {
                        continue;
                    }

                    warnings.push(warning as &dyn miette::Diagnostic);
                }
                for warning in result.warnings.iter() {
                    if warning_module_disables(warning.source_code(), &quiet_dirs) {
                        continue;
                    }

                    warnings.push(warning as &dyn miette::Diagnostic);
                }

                let count = warnings.len();
                for warning in warnings {
                    let report_string = driver.report(warning).unwrap_or_default();
                    eprintln!("{}", report_string);
                }

                if count > 0 {
                    cli::println_warn(format!("Compiler reported {count} warning(s)"));
                }
            }

            if args.check {
                cli::println_info(
                    "Check",
                    format!(
                        "all checks passed (in {:.2}s)",
                        compilation_start.elapsed().as_secs_f32()
                    ),
                );

                exit(0);
            };
        }
        Err(errors) => {
            for err in &errors {
                let report_string = driver.report(err).unwrap_or_default();
                eprintln!("{}", report_string);
            }

            cli::println_error(format!("Compiler returned {} error(s)", errors.len()));

            exit(1);
        }
    }

    if args.emit == CompilationOutput::EmitMIR {
        let printed_mir = zeen_mir::printer::print_mir_program(
            &lowered_mir.program,
            &typechecker_result,
            &resolution_result,
            &rodeo,
        );

        let output_path = with_default_extension(&output, "mir");

        let mut output_file = std::fs::File::create(&output_path).unwrap_or_else(|_| {
            cli::println_warn("Unable to write MIR to file, printing to stdout...");
            println!("\n{}", printed_mir);

            exit(0);
        });

        output_file
            .write_all(printed_mir.as_bytes())
            .unwrap_or_else(|_| {
                cli::println_warn("Unable to write MIR to file, printing to stdout...");
                println!("\n{}", printed_mir);

                exit(0);
            });

        cli::println_info(
            "Emitted",
            format!("MIR to the file ({})", output_path.display()),
        );

        exit(0);
    }

    // cli::println_info("Generating", "LLVM IR from MIR");

    let codegen_options = zeen_codegen_llvm::CodegenOptions {
        mode: context.mode,
        target: Some(target_triple.clone()),
        main_fn: lowered_mir.main_fn,
        source_file_name: filename.to_string(),
        ..Default::default()
    };

    let mut linked_files: Vec<std::path::PathBuf> = context.paths.linked.iter().cloned().collect();
    linked_files.sort();

    let units = match args.emit {
        CompilationOutput::Binary if !zeen_driver::Target::parse(&target_triple).is_wasm() => args
            .codegen_units
            .resolve(lowered_mir.program.functions.len()),
        _ => 1,
    };

    let names = zeen_codegen_llvm::CodeGenNames::snapshot(&rodeo.borrow());

    match args.emit {
        CompilationOutput::EmitIR => {
            let output_path = with_default_extension(&output, "ll");

            let result = run_codegen_unit(
                &lowered_mir.program,
                &typechecker_result,
                &resolution_result,
                &names,
                codegen_options.clone(),
                |codegen| {
                    timer.mark("codegen");
                    codegen.emit_ir(&output_path)
                },
            );

            if let Err(err) = result {
                let report_string = driver.report(&err).unwrap_or_default();
                eprintln!("{}", report_string);
                cli::println_error("Codegen failed");
                exit(1);
            }

            timer.mark("emit");

            cli::println_info(
                "Emitted",
                format!("LLVM IR to the file ({})", output_path.display()),
            );

            cli::println_info(
                "Finished",
                format!(
                    "`{}` profile [{}] in {:.2}s",
                    args.mode,
                    target_triple,
                    compilation_start.elapsed().as_secs_f32()
                ),
            );
        }

        CompilationOutput::Object => {
            let output_path = with_default_extension(
                &output,
                zeen_linker::ObjectLinker::object_extension_for(&target_triple),
            );

            let result = run_codegen_unit(
                &lowered_mir.program,
                &typechecker_result,
                &resolution_result,
                &names,
                codegen_options.clone(),
                |codegen| {
                    timer.mark("codegen");
                    codegen.emit_object(&output_path)
                },
            );

            if let Err(err) = result {
                let report_string = driver.report(&err).unwrap_or_default();
                eprintln!("{}", report_string);
                cli::println_error("Codegen failed");
                exit(1);
            }

            timer.mark("emit");

            cli::println_info(
                "Emitted",
                format!("object file to the file ({})", output_path.display()),
            );

            cli::println_info(
                "Finished",
                format!(
                    "`{}` profile [{}] in {:.2}s",
                    args.mode,
                    target_triple,
                    compilation_start.elapsed().as_secs_f32()
                ),
            );
        }

        CompilationOutput::Binary => {
            let mut linker =
                zeen_linker::ObjectLinker::detect(&target_triple).unwrap_or_else(|err| {
                    cli::println_error(err);
                    exit(1);
                });

            if let Some(linker_path) = args.linker_path.as_deref() {
                let Some(program) = linker_path.to_str() else {
                    cli::println_error(format!(
                        "Linker path is not valid UTF-8: `{}`",
                        linker_path.display()
                    ));
                    exit(1);
                };
                linker.with_linker(program);
            }

            linker.with_link_libs(&args.link);
            linker.with_link_dirs(&args.link_dir);
            linker.with_options(zeen_linker::LinkOptions {
                strip: matches!(args.mode, CompilationMode::Release),
                small: args.small,
                gc_sections: matches!(args.mode, CompilationMode::Debug),
            });

            let object_files: Vec<tempfile::TempPath> = (0..units)
                .map(|_| {
                    tempfile::Builder::new()
                        .prefix("zeen-")
                        .suffix(&format!(".{}", linker.object_extension()))
                        .tempfile()
                        .unwrap_or_else(|err| {
                            cli::println_error(format!("Failed to create temporary file: {err}"));
                            exit(1);
                        })
                        .into_temp_path()
                })
                .collect();
            let object_paths: Vec<std::path::PathBuf> =
                object_files.iter().map(|path| path.to_path_buf()).collect();

            let mut codegen_time = std::time::Duration::ZERO;
            let mut emit_time = std::time::Duration::ZERO;

            if units == 1 {
                let started = std::time::Instant::now();
                let result = run_codegen_unit(
                    &lowered_mir.program,
                    &typechecker_result,
                    &resolution_result,
                    &names,
                    codegen_options.clone(),
                    |codegen| {
                        let generated = started.elapsed();
                        let emit_started = std::time::Instant::now();
                        codegen.emit_object(&object_paths[0])?;
                        Ok((generated, emit_started.elapsed()))
                    },
                );

                match result {
                    Ok((generated, emit)) => {
                        codegen_time = generated;
                        emit_time = emit;
                    }
                    Err(err) => {
                        let report_string = driver.report(&err).unwrap_or_default();
                        eprintln!("{}", report_string);
                        cli::println_error("Codegen failed");
                        exit(1);
                    }
                }
            } else {
                let mut failures = Vec::new();

                std::thread::scope(|scope| {
                    let mut handles = Vec::new();

                    for (index, path) in object_paths.iter().enumerate() {
                        let options = zeen_codegen_llvm::CodegenOptions {
                            units,
                            unit: index as u32,
                            ..codegen_options.clone()
                        };
                        let names = &names;
                        let program = &lowered_mir.program;
                        let typecheck = &typechecker_result;
                        let resolution = &resolution_result;

                        let handle = std::thread::Builder::new()
                            .stack_size(COMPILER_THREAD_STACK)
                            .spawn_scoped(scope, move || {
                                let started = std::time::Instant::now();
                                run_codegen_unit(
                                    program,
                                    typecheck,
                                    resolution,
                                    names,
                                    options,
                                    |codegen| {
                                        let generated = started.elapsed();
                                        let emit_started = std::time::Instant::now();
                                        codegen.emit_object(path)?;
                                        Ok((generated, emit_started.elapsed()))
                                    },
                                )
                            })
                            .unwrap_or_else(|err| {
                                cli::println_error(format!(
                                    "Failed to spawn codegen thread: {err}"
                                ));
                                exit(1);
                            });

                        handles.push(handle);
                    }

                    for handle in handles {
                        match handle.join() {
                            Ok(Ok((generated, emit))) => {
                                codegen_time = codegen_time.max(generated);
                                emit_time = emit_time.max(emit);
                            }
                            Ok(Err(err)) => failures.push(err),
                            Err(_) => cli::println_error("Codegen thread panicked"),
                        }
                    }
                });

                if !failures.is_empty() {
                    for err in &failures {
                        let report_string = driver.report(err).unwrap_or_default();
                        eprintln!("{}", report_string);
                    }
                    cli::println_error("Codegen failed");
                    exit(1);
                }
            }

            timer.mark_elapsed("codegen", codegen_time);
            timer.mark_elapsed("emit", emit_time);

            let extra: Vec<std::path::PathBuf> = if linker.accepts_c_sources() {
                linked_files.clone()
            } else {
                if !linked_files.is_empty() {
                    cli::println_warn(
                        "skipped `extern link` sources: this toolchain only links object files",
                    );
                }
                Vec::new()
            };

            let result = linker.link(&object_paths, &output, &extra);

            timer.mark("link");

            match result {
                Ok(output_path) => {
                    cli::println_info(
                        "Emitted",
                        format!(
                            "binary (with {}): `{}`",
                            linker.name(),
                            output_path.display()
                        ),
                    );

                    cli::println_info(
                        "Finished",
                        format!(
                            "`{}` profile [{}] in {:.2}s",
                            args.mode,
                            target_triple,
                            compilation_start.elapsed().as_secs_f32()
                        ),
                    );
                }
                Err(err) => {
                    cli::println_error(format!(
                        "Linker failed (object linker: `{}`)",
                        linker.name()
                    ));
                    eprintln!("\n{err}\n");
                    drop(object_files);
                    exit(1);
                }
            }
        }

        CompilationOutput::EmitMIR => unreachable!("handled above"),
    }
}
