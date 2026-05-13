use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{self, Command};

type XtaskResult<T> = Result<T, String>;

const DOC_ROOT: &str = "universal_arb_platform_v2_immutable_core_docs";
const REQUIRED_TEXT_DOCS: &[&str] = &[
    "AGENTS.md",
    "universal_arb_platform_v2_immutable_core_docs/README.md",
];
const FIXTURE_DOCS: &[&str] = &[
    "fixtures/schema/valid/README.md",
    "fixtures/schema/invalid/README.md",
    "fixtures/replay/README.md",
];

const BOUNDARY_RULES: &[BoundaryRule] = &[
    BoundaryRule::forbid(
        "arb-strategies",
        "arb-execution",
        "策略实现不能获得执行计划或模拟执行内部能力。",
    ),
    BoundaryRule::forbid(
        "arb-strategies",
        "arb-venue-exec",
        "策略实现不能依赖可变场所执行适配器。",
    ),
    BoundaryRule::forbid(
        "arb-strategies",
        "arb-signing",
        "策略实现不能依赖签名边界。",
    ),
    BoundaryRule::forbid(
        "arb-strategies",
        "arb-ledger",
        "策略实现不能写入或直接耦合账本。",
    ),
    BoundaryRule::forbid(
        "arb-strategies",
        "arb-runtime",
        "策略实现不能依赖运行时装配层。",
    ),
    BoundaryRule::forbid(
        "arb-strategy-api",
        "arb-execution",
        "策略 API 只能暴露只读接口，不能暴露执行能力。",
    ),
    BoundaryRule::forbid(
        "arb-strategy-api",
        "arb-venue-exec",
        "策略 API 不能暴露可变场所执行能力。",
    ),
    BoundaryRule::forbid(
        "arb-strategy-api",
        "arb-signing",
        "策略 API 不能暴露签名能力。",
    ),
    BoundaryRule::forbid(
        "arb-strategy-api",
        "arb-ledger",
        "策略 API 不能暴露账本写入能力。",
    ),
    BoundaryRule::forbid(
        "arb-strategy-api",
        "arb-runtime",
        "策略 API 不能依赖运行时装配层。",
    ),
    BoundaryRule::forbid(
        "arb-risk",
        "arb-execution",
        "风控只输出决策，不能调度执行。",
    ),
    BoundaryRule::forbid(
        "arb-risk",
        "arb-venue-exec",
        "风控不能调用可变场所执行适配器。",
    ),
    BoundaryRule::forbid("arb-risk", "arb-signing", "风控不能依赖签名边界。"),
    BoundaryRule::forbid("arb-risk", "arb-runtime", "风控不能依赖运行时装配层。"),
    BoundaryRule::forbid(
        "arb-ledger",
        "arb-risk",
        "账本只记录事实，不能调用风控规则。",
    ),
    BoundaryRule::forbid("arb-ledger", "arb-execution", "账本不能调用执行模块。"),
    BoundaryRule::forbid(
        "arb-ledger",
        "arb-venue-exec",
        "账本不能调用可变场所执行适配器。",
    ),
    BoundaryRule::forbid("arb-ledger", "arb-signing", "账本不能读取或触发签名。"),
    BoundaryRule::forbid("arb-ledger", "arb-runtime", "账本不能依赖运行时装配层。"),
    BoundaryRule::forbid(
        "arb-venue-data",
        "arb-venue-exec",
        "只读场所数据适配器不能依赖可变执行适配器。",
    ),
    BoundaryRule::forbid(
        "arb-venue-data",
        "arb-signing",
        "只读场所数据适配器不能依赖签名边界。",
    ),
    BoundaryRule::forbid(
        "arb-venue-data",
        "arb-strategies",
        "只读场所数据适配器不能依赖策略实现。",
    ),
    BoundaryRule::forbid_features(
        "arb-ops",
        "arb-venue-exec",
        &["live-exec"],
        "运营模块不能启用可变适配器的实盘实现。",
    ),
    BoundaryRule::forbid_features(
        "arb-ops",
        "arb-signing",
        &["real-signing"],
        "运营模块不能启用真实签名实现。",
    ),
    BoundaryRule::forbid_features(
        "arb-replay",
        "arb-venue-exec",
        &["live-exec"],
        "回放模块不能启用实盘账户写入能力。",
    ),
    BoundaryRule::forbid_features(
        "arb-replay",
        "arb-signing",
        &["real-signing"],
        "回放模块不能启用真实签名能力。",
    ),
];

fn main() {
    if let Err(error) = run() {
        eprintln!("xtask failed: {error}");
        process::exit(1);
    }
}

fn run() -> XtaskResult<()> {
    let mut args = env::args().skip(1);
    let command = args.next().unwrap_or_else(|| "help".to_owned());
    let root = repo_root()?;

    match command.as_str() {
        "check-schema" => check_schema(&root),
        "check-crate-boundaries" => check_crate_boundaries(&root),
        "check-docs" => check_docs(&root),
        "guarded-live-preflight" => {
            let config_path = args
                .next()
                .unwrap_or_else(|| "templates/personal_guarded_live.preflight.yaml".to_owned());
            if let Some(extra) = args.next() {
                return Err(format!(
                    "guarded-live-preflight accepts at most one config path, got extra `{extra}`"
                ));
            }
            run_guarded_live_preflight(&root, &config_path)
        }
        "replay-full-pipeline" => run_cargo_command(
            &root,
            &[
                "run",
                "-p",
                "arb-runtime",
                "--",
                "replay",
                "fixtures/replay/full_pipeline_simulated",
            ],
        ),
        "quality-gate" => {
            check_schema(&root)?;
            check_crate_boundaries(&root)?;
            check_docs(&root)
        }
        "help" | "-h" | "--help" => {
            print_help();
            Ok(())
        }
        other => Err(format!(
            "unknown command `{other}`. Supported commands: check-schema, check-crate-boundaries, check-docs, guarded-live-preflight, replay-full-pipeline, quality-gate"
        )),
    }
}

fn print_help() {
    println!("easy-arb xtask");
    println!("中文说明：阶段 0 本地检查入口，只做离线、只读检查。");
    println!();
    println!("Commands:");
    println!("  check-schema            Parse schema JSON and schema fixture JSON files");
    println!(
        "  check-crate-boundaries  Check cargo metadata against built-in forbidden dependencies"
    );
    println!("  check-docs              Check guidance notes and Chinese fixture notes");
    println!(
        "  guarded-live-preflight  Check a local personal guarded-live config without credentials"
    );
    println!("  replay-full-pipeline    Run the Stage 9 full pipeline replay fixture");
    println!("  quality-gate            Run all current xtask checks");
}

fn run_guarded_live_preflight(root: &Path, config_path: &str) -> XtaskResult<()> {
    println!("guarded-live-preflight: {config_path}");
    println!(
        "中文说明：该命令只读取本地配置并运行启动检查，不访问网络、不读取凭证、不提交真实账户动作。"
    );
    run_cargo_command(
        root,
        &[
            "run",
            "-p",
            "arb-runtime",
            "--",
            "health-config",
            config_path,
        ],
    )
}

fn run_cargo_command(root: &Path, args: &[&str]) -> XtaskResult<()> {
    println!("cargo {}", args.join(" "));
    println!("中文说明：该命令只运行离线或只读检查，不访问真实交易 API 或真实凭证。");
    let status = Command::new("cargo")
        .args(args)
        .current_dir(root)
        .status()
        .map_err(|error| format!("cannot run cargo {}: {error}", args.join(" ")))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "cargo {} failed with status {status}",
            args.join(" ")
        ))
    }
}

fn check_schema(root: &Path) -> XtaskResult<()> {
    println!("check-schema: parsing schema JSON files");
    println!("中文说明：S0-02 只检查 JSON 语法和目录存在性，合同字段校验在后续阶段实现。");

    let schema_dir = root.join(DOC_ROOT).join("schemas");
    ensure_dir(&schema_dir)?;

    let valid_dir = root.join("fixtures/schema/valid");
    let invalid_dir = root.join("fixtures/schema/invalid");
    ensure_dir(&valid_dir)?;
    ensure_dir(&invalid_dir)?;

    let schema_files = collect_json_files(&schema_dir)?;
    if schema_files.is_empty() {
        return Err(format!(
            "no schema JSON files found under {}",
            rel(root, &schema_dir)
        ));
    }

    for file in &schema_files {
        parse_json_file(file).map_err(|error| format!("{}: {error}", rel(root, file)))?;
    }

    let valid_fixtures = collect_json_files(&valid_dir)?;
    for file in &valid_fixtures {
        parse_json_file(file).map_err(|error| format!("{}: {error}", rel(root, file)))?;
    }

    let invalid_fixtures = collect_json_files(&invalid_dir)?;
    for file in &invalid_fixtures {
        parse_json_file(file).map_err(|error| format!("{}: {error}", rel(root, file)))?;
    }

    println!(
        "ok: parsed {} schema files, {} valid fixture JSON files, {} invalid fixture JSON files",
        schema_files.len(),
        valid_fixtures.len(),
        invalid_fixtures.len()
    );
    if valid_fixtures.is_empty() || invalid_fixtures.is_empty() {
        println!("note: schema fixture JSON files are allowed to be empty in S0-02 scaffold.");
    }
    Ok(())
}

fn check_crate_boundaries(root: &Path) -> XtaskResult<()> {
    println!("check-crate-boundaries: reading cargo metadata");
    println!("中文说明：根据 xtask 内置禁止依赖表检查 workspace crate 的直接依赖。");

    let packages = read_workspace_metadata(root)?;
    validate_required_boundary_packages(&packages)?;
    let violations = boundary_violations(&packages);
    if !violations.is_empty() {
        return Err(format!(
            "crate boundary violations found:\n{}",
            violations.join("\n")
        ));
    }

    println!(
        "ok: checked {} forbidden dependency rules across {} workspace packages",
        BOUNDARY_RULES.len(),
        packages.len()
    );
    Ok(())
}

fn check_docs(root: &Path) -> XtaskResult<()> {
    println!("check-docs: checking repository guidance notes");
    println!(
        "中文说明：文档检查当前覆盖 AGENTS.md、资料入口和 fixture 说明的存在性及中文说明要求。"
    );

    for doc in REQUIRED_TEXT_DOCS {
        let path = root.join(doc);
        ensure_file(&path)?;
        let contents = read_utf8(&path)?;
        if !contains_chinese(&contents) {
            return Err(format!(
                "{} does not contain Chinese text",
                rel(root, &path)
            ));
        }
    }

    for doc in FIXTURE_DOCS {
        let path = root.join(doc);
        ensure_file(&path)?;
        let contents = read_utf8(&path)?;
        if !contains_chinese(&contents) {
            return Err(format!(
                "{} does not contain Chinese text",
                rel(root, &path)
            ));
        }
    }

    println!(
        "ok: checked {} guidance notes and {} fixture notes",
        REQUIRED_TEXT_DOCS.len(),
        FIXTURE_DOCS.len()
    );
    Ok(())
}

fn repo_root() -> XtaskResult<PathBuf> {
    let mut current =
        env::current_dir().map_err(|error| format!("cannot read current dir: {error}"))?;
    loop {
        if current.join("Cargo.toml").is_file() && current.join(DOC_ROOT).is_dir() {
            return Ok(current);
        }
        if !current.pop() {
            return Err(format!(
                "cannot find repository root containing Cargo.toml and {DOC_ROOT}"
            ));
        }
    }
}

fn collect_json_files(dir: &Path) -> XtaskResult<Vec<PathBuf>> {
    collect_files(dir, |path| {
        path.extension().is_some_and(|ext| ext == "json")
    })
}

fn collect_files(dir: &Path, keep: impl Fn(&Path) -> bool) -> XtaskResult<Vec<PathBuf>> {
    ensure_dir(dir)?;
    let mut files = Vec::new();
    collect_files_inner(dir, &keep, &mut files)?;
    files.sort();
    Ok(files)
}

fn collect_files_inner(
    dir: &Path,
    keep: &impl Fn(&Path) -> bool,
    files: &mut Vec<PathBuf>,
) -> XtaskResult<()> {
    for entry in
        fs::read_dir(dir).map_err(|error| format!("cannot read {}: {error}", dir.display()))?
    {
        let entry = entry
            .map_err(|error| format!("cannot read dir entry in {}: {error}", dir.display()))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|error| format!("cannot read file type for {}: {error}", path.display()))?;
        if file_type.is_dir() {
            collect_files_inner(&path, keep, files)?;
        } else if file_type.is_file() && keep(&path) {
            files.push(path);
        }
    }
    Ok(())
}

fn parse_json_file(path: &Path) -> XtaskResult<()> {
    let contents = read_utf8(path)?;
    JsonParser::new(&contents).parse().map(|_| ())
}

fn read_utf8(path: &Path) -> XtaskResult<String> {
    fs::read_to_string(path)
        .map_err(|error| format!("cannot read {} as UTF-8: {error}", path.display()))
}

fn ensure_file(path: &Path) -> XtaskResult<()> {
    if path.is_file() {
        Ok(())
    } else {
        Err(format!("missing file {}", path.display()))
    }
}

fn ensure_dir(path: &Path) -> XtaskResult<()> {
    if path.is_dir() {
        Ok(())
    } else {
        Err(format!("missing directory {}", path.display()))
    }
}

fn contains_chinese(contents: &str) -> bool {
    contents
        .chars()
        .any(|ch| ('\u{4e00}'..='\u{9fff}').contains(&ch))
}

fn rel(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

#[derive(Debug)]
struct BoundaryRule {
    dependent: &'static str,
    forbidden: &'static str,
    features: &'static [&'static str],
    reason: &'static str,
}

impl BoundaryRule {
    const fn forbid(
        dependent: &'static str,
        forbidden: &'static str,
        reason: &'static str,
    ) -> Self {
        Self {
            dependent,
            forbidden,
            features: &[],
            reason,
        }
    }

    const fn forbid_features(
        dependent: &'static str,
        forbidden: &'static str,
        features: &'static [&'static str],
        reason: &'static str,
    ) -> Self {
        Self {
            dependent,
            forbidden,
            features,
            reason,
        }
    }
}

#[derive(Debug)]
struct PackageMetadata {
    name: String,
    dependencies: Vec<DependencyMetadata>,
}

#[derive(Debug)]
struct DependencyMetadata {
    name: String,
    features: Vec<String>,
}

impl DependencyMetadata {
    fn matches_rule(&self, rule: &BoundaryRule) -> bool {
        if self.name != rule.forbidden {
            return false;
        }

        rule.features.is_empty()
            || rule
                .features
                .iter()
                .any(|feature| self.features.iter().any(|enabled| enabled == feature))
    }
}

fn read_workspace_metadata(root: &Path) -> XtaskResult<BTreeMap<String, PackageMetadata>> {
    ensure_file(&root.join("Cargo.toml"))?;

    let output = Command::new("cargo")
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .current_dir(root)
        .output()
        .map_err(|error| format!("cannot run cargo metadata: {error}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("cargo metadata failed: {}", stderr.trim()));
    }

    let stdout = String::from_utf8(output.stdout)
        .map_err(|error| format!("cargo metadata emitted non UTF-8 stdout: {error}"))?;
    let metadata = JsonParser::new(&stdout)
        .parse()
        .map_err(|error| format!("cannot parse cargo metadata JSON: {error}"))?;
    parse_workspace_packages(&metadata)
}

fn parse_workspace_packages(
    metadata: &JsonValue,
) -> XtaskResult<BTreeMap<String, PackageMetadata>> {
    let root = expect_object(metadata, "cargo metadata root")?;
    let workspace_members = required_string_set(root, "workspace_members")?;
    let packages = required_array(root, "packages")?;
    let mut by_name = BTreeMap::new();

    for package in packages {
        let package = expect_object(package, "cargo metadata package")?;
        let id = required_string(package, "id")?;
        if !workspace_members.contains(&id) {
            continue;
        }

        let name = required_string(package, "name")?;
        let dependencies = parse_dependencies(required_array(package, "dependencies")?)?;
        by_name.insert(name.clone(), PackageMetadata { name, dependencies });
    }

    if by_name.is_empty() {
        return Err("cargo metadata returned no workspace packages".to_owned());
    }

    Ok(by_name)
}

fn parse_dependencies(values: &[JsonValue]) -> XtaskResult<Vec<DependencyMetadata>> {
    let mut dependencies = Vec::new();
    for value in values {
        let dependency = expect_object(value, "cargo metadata dependency")?;
        let name = required_string(dependency, "name")?;
        let features = optional_string_vec(dependency, "features")?;
        dependencies.push(DependencyMetadata { name, features });
    }
    Ok(dependencies)
}

fn validate_required_boundary_packages(
    packages: &BTreeMap<String, PackageMetadata>,
) -> XtaskResult<()> {
    let required = BOUNDARY_RULES
        .iter()
        .map(|rule| rule.dependent)
        .collect::<BTreeSet<_>>();
    let missing = required
        .into_iter()
        .filter(|package| !packages.contains_key(*package))
        .collect::<Vec<_>>();

    if missing.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "workspace is missing packages required by boundary rules: {}",
            missing.join(", ")
        ))
    }
}

fn boundary_violations(packages: &BTreeMap<String, PackageMetadata>) -> Vec<String> {
    let mut violations = Vec::new();

    for rule in BOUNDARY_RULES {
        let Some(package) = packages.get(rule.dependent) else {
            continue;
        };

        for dependency in &package.dependencies {
            if dependency.matches_rule(rule) {
                violations.push(format!(
                    "- {} 不能依赖 {}{}：{}",
                    package.name,
                    rule.forbidden,
                    feature_phrase(rule.features),
                    rule.reason
                ));
            }
        }
    }

    violations
}

fn feature_phrase(features: &[&str]) -> String {
    if features.is_empty() {
        String::new()
    } else {
        format!(" with features [{}]", features.join(", "))
    }
}

fn expect_object<'a>(
    value: &'a JsonValue,
    context: &str,
) -> XtaskResult<&'a BTreeMap<String, JsonValue>> {
    match value {
        JsonValue::Object(object) => Ok(object),
        _ => Err(format!("{context} must be a JSON object")),
    }
}

fn required_array<'a>(
    object: &'a BTreeMap<String, JsonValue>,
    field: &str,
) -> XtaskResult<&'a [JsonValue]> {
    match object.get(field) {
        Some(JsonValue::Array(values)) => Ok(values),
        Some(_) => Err(format!("cargo metadata field `{field}` must be an array")),
        None => Err(format!("cargo metadata missing field `{field}`")),
    }
}

fn required_string(object: &BTreeMap<String, JsonValue>, field: &str) -> XtaskResult<String> {
    match object.get(field) {
        Some(JsonValue::String(value)) => Ok(value.clone()),
        Some(_) => Err(format!("cargo metadata field `{field}` must be a string")),
        None => Err(format!("cargo metadata missing field `{field}`")),
    }
}

fn required_string_set(
    object: &BTreeMap<String, JsonValue>,
    field: &str,
) -> XtaskResult<BTreeSet<String>> {
    let values = required_array(object, field)?;
    let mut set = BTreeSet::new();
    for value in values {
        match value {
            JsonValue::String(value) => {
                set.insert(value.clone());
            }
            _ => {
                return Err(format!(
                    "cargo metadata field `{field}` must contain strings"
                ))
            }
        }
    }
    Ok(set)
}

fn optional_string_vec(
    object: &BTreeMap<String, JsonValue>,
    field: &str,
) -> XtaskResult<Vec<String>> {
    let Some(value) = object.get(field) else {
        return Ok(Vec::new());
    };

    match value {
        JsonValue::Array(values) => {
            let mut strings = Vec::new();
            for value in values {
                match value {
                    JsonValue::String(value) => strings.push(value.clone()),
                    _ => {
                        return Err(format!(
                            "cargo metadata field `{field}` must contain strings"
                        ))
                    }
                }
            }
            Ok(strings)
        }
        _ => Err(format!("cargo metadata field `{field}` must be an array")),
    }
}

#[derive(Debug, PartialEq)]
enum JsonValue {
    Null,
    Bool,
    Number,
    String(String),
    Array(Vec<JsonValue>),
    Object(BTreeMap<String, JsonValue>),
}

struct JsonParser<'a> {
    input: &'a [u8],
    position: usize,
}

impl<'a> JsonParser<'a> {
    fn new(input: &'a str) -> Self {
        Self {
            input: input.as_bytes(),
            position: 0,
        }
    }

    fn parse(mut self) -> XtaskResult<JsonValue> {
        self.skip_whitespace();
        let value = self.parse_value()?;
        self.skip_whitespace();
        if self.position == self.input.len() {
            Ok(value)
        } else {
            self.error("trailing characters after JSON value")
        }
    }

    fn parse_value(&mut self) -> XtaskResult<JsonValue> {
        self.skip_whitespace();
        match self.peek() {
            Some(b'{') => self.parse_object(),
            Some(b'[') => self.parse_array(),
            Some(b'"') => self.parse_string().map(JsonValue::String),
            Some(b'-' | b'0'..=b'9') => self.parse_number(),
            Some(b't') => self.consume_literal(b"true", JsonValue::Bool),
            Some(b'f') => self.consume_literal(b"false", JsonValue::Bool),
            Some(b'n') => self.consume_literal(b"null", JsonValue::Null),
            Some(byte) => self.error(&format!("unexpected byte `{}`", byte as char)),
            None => self.error("unexpected end of input"),
        }
    }

    fn parse_object(&mut self) -> XtaskResult<JsonValue> {
        self.expect(b'{')?;
        self.skip_whitespace();
        if self.consume_if(b'}') {
            return Ok(JsonValue::Object(BTreeMap::new()));
        }

        let mut object = BTreeMap::new();
        loop {
            self.skip_whitespace();
            let key = self.parse_string()?;
            self.skip_whitespace();
            self.expect(b':')?;
            let value = self.parse_value()?;
            object.insert(key, value);
            self.skip_whitespace();

            if self.consume_if(b'}') {
                return Ok(JsonValue::Object(object));
            }
            self.expect(b',')?;
        }
    }

    fn parse_array(&mut self) -> XtaskResult<JsonValue> {
        self.expect(b'[')?;
        self.skip_whitespace();
        if self.consume_if(b']') {
            return Ok(JsonValue::Array(Vec::new()));
        }

        let mut values = Vec::new();
        loop {
            values.push(self.parse_value()?);
            self.skip_whitespace();
            if self.consume_if(b']') {
                return Ok(JsonValue::Array(values));
            }
            self.expect(b',')?;
        }
    }

    fn parse_string(&mut self) -> XtaskResult<String> {
        self.expect(b'"')?;
        let mut string = String::new();
        let mut segment_start = self.position;

        while let Some(byte) = self.peek() {
            match byte {
                b'"' => {
                    self.push_utf8_segment(segment_start, self.position, &mut string)?;
                    self.position += 1;
                    return Ok(string);
                }
                b'\\' => {
                    self.push_utf8_segment(segment_start, self.position, &mut string)?;
                    self.position += 1;
                    string.push(self.parse_escape()?);
                    segment_start = self.position;
                }
                0x00..=0x1f => return self.error("control character in JSON string"),
                _ => self.position += 1,
            }
        }
        self.error("unterminated JSON string")
    }

    fn push_utf8_segment(&self, start: usize, end: usize, output: &mut String) -> XtaskResult<()> {
        let segment = std::str::from_utf8(&self.input[start..end])
            .map_err(|error| format!("invalid UTF-8 in JSON string at byte {start}: {error}"))?;
        output.push_str(segment);
        Ok(())
    }

    fn parse_escape(&mut self) -> XtaskResult<char> {
        match self.next() {
            Some(b'"') => Ok('"'),
            Some(b'\\') => Ok('\\'),
            Some(b'/') => Ok('/'),
            Some(b'b') => Ok('\u{0008}'),
            Some(b'f') => Ok('\u{000c}'),
            Some(b'n') => Ok('\n'),
            Some(b'r') => Ok('\r'),
            Some(b't') => Ok('\t'),
            Some(b'u') => self.parse_unicode_escape(),
            Some(byte) => self.error(&format!("invalid JSON escape `{}`", byte as char)),
            None => self.error("unterminated JSON escape"),
        }
    }

    fn parse_unicode_escape(&mut self) -> XtaskResult<char> {
        let mut value = 0_u32;
        for _ in 0..4 {
            let Some(byte) = self.next() else {
                return self.error("unterminated unicode escape in JSON string");
            };
            let Some(digit) = (byte as char).to_digit(16) else {
                return self.error("invalid unicode escape in JSON string");
            };
            value = value * 16 + digit;
        }

        char::from_u32(value).ok_or_else(|| {
            format!(
                "invalid unicode scalar value in JSON string at byte {}",
                self.position
            )
        })
    }

    fn parse_number(&mut self) -> XtaskResult<JsonValue> {
        self.consume_if(b'-');

        match self.peek() {
            Some(b'0') => {
                self.position += 1;
            }
            Some(b'1'..=b'9') => {
                self.position += 1;
                self.consume_digits();
            }
            _ => return self.error("invalid JSON number"),
        }

        if self.consume_if(b'.') && !self.consume_one_or_more_digits() {
            return self.error("JSON number fraction requires digits");
        }

        if self.consume_if(b'e') || self.consume_if(b'E') {
            let _ = self.consume_if(b'+') || self.consume_if(b'-');
            if !self.consume_one_or_more_digits() {
                return self.error("JSON number exponent requires digits");
            }
        }

        Ok(JsonValue::Number)
    }

    fn consume_literal(&mut self, literal: &[u8], value: JsonValue) -> XtaskResult<JsonValue> {
        if self.input.get(self.position..self.position + literal.len()) == Some(literal) {
            self.position += literal.len();
            Ok(value)
        } else {
            self.error("invalid JSON literal")
        }
    }

    fn consume_digits(&mut self) {
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.position += 1;
        }
    }

    fn consume_one_or_more_digits(&mut self) -> bool {
        let start = self.position;
        self.consume_digits();
        self.position > start
    }

    fn skip_whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\n' | b'\r' | b'\t')) {
            self.position += 1;
        }
    }

    fn expect(&mut self, expected: u8) -> XtaskResult<()> {
        match self.next() {
            Some(byte) if byte == expected => Ok(()),
            Some(byte) => self.error(&format!(
                "expected `{}`, got `{}`",
                expected as char, byte as char
            )),
            None => self.error(&format!(
                "expected `{}`, got end of input",
                expected as char
            )),
        }
    }

    fn consume_if(&mut self, expected: u8) -> bool {
        if self.peek() == Some(expected) {
            self.position += 1;
            true
        } else {
            false
        }
    }

    fn peek(&self) -> Option<u8> {
        self.input.get(self.position).copied()
    }

    fn next(&mut self) -> Option<u8> {
        let byte = self.peek()?;
        self.position += 1;
        Some(byte)
    }

    fn error<T>(&self, message: &str) -> XtaskResult<T> {
        Err(format!("{message} at byte {}", self.position))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        boundary_violations, DependencyMetadata, JsonParser, PackageMetadata, BOUNDARY_RULES,
    };
    use std::collections::BTreeMap;

    #[test]
    fn parses_nested_json() {
        let value =
            JsonParser::new(r#"{"schema_version":"1","items":[true,false,null,{"n":-12.5e+2}]}"#)
                .parse()
                .expect("valid JSON should parse");
        assert!(matches!(value, super::JsonValue::Object(_)));
    }

    #[test]
    fn rejects_trailing_comma() {
        assert!(JsonParser::new(r#"{"a":1,}"#).parse().is_err());
    }

    #[test]
    fn boundary_rules_cover_required_s0_03_modules() {
        let covered = BOUNDARY_RULES
            .iter()
            .map(|rule| rule.dependent)
            .collect::<std::collections::BTreeSet<_>>();

        for package in [
            "arb-strategies",
            "arb-strategy-api",
            "arb-risk",
            "arb-ledger",
            "arb-venue-data",
        ] {
            assert!(covered.contains(package), "{package} must be covered");
        }
    }

    #[test]
    fn detects_forbidden_strategy_dependency() {
        let packages = BTreeMap::from([(
            "arb-strategies".to_owned(),
            PackageMetadata {
                name: "arb-strategies".to_owned(),
                dependencies: vec![DependencyMetadata {
                    name: "arb-execution".to_owned(),
                    features: Vec::new(),
                }],
            },
        )]);

        let violations = boundary_violations(&packages);
        assert_eq!(violations.len(), 1);
        assert!(violations[0].contains("arb-strategies"));
        assert!(violations[0].contains("arb-execution"));
    }

    #[test]
    fn detects_forbidden_live_feature_dependency() {
        let packages = BTreeMap::from([(
            "arb-ops".to_owned(),
            PackageMetadata {
                name: "arb-ops".to_owned(),
                dependencies: vec![DependencyMetadata {
                    name: "arb-venue-exec".to_owned(),
                    features: vec!["live-exec".to_owned()],
                }],
            },
        )]);

        let violations = boundary_violations(&packages);
        assert_eq!(violations.len(), 1);
        assert!(violations[0].contains("live-exec"));
    }
}
