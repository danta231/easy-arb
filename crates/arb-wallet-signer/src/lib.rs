//! 本地钱包签名工具。
//!
//! 中文说明：该 crate 只负责把已受控的交易所签名 payload 转成 ECDSA 签名。
//! 它不访问交易所、不提交订单、不写入密钥，并且错误信息不能包含私钥原文。

#![forbid(unsafe_code)]

use libsecp256k1::{Message, PublicKey, SecretKey};
use serde_json::Value;
use sha3::{Digest, Keccak256};
use std::fmt;

const ASTER_KEY_ENV_DEFAULT: &str = "ASTER_SIGNER_PRIVATE_KEY";
const ASTER_KEY_ENV_ALIASES: &[&str] = &["ASTER_SIGNER_PRIVATE", "ASTER_PRIVATE_KEY"];
const HYPERLIQUID_KEY_ENV_DEFAULT: &str = "HYPERLIQUID_AGENT_PRIVATE_KEY";
const HYPERLIQUID_KEY_ENV_ALIASES: &[&str] = &[
    "HYPERLIQUID_SIGNER_PRIVATE_KEY",
    "HYPERLIQUID_SIGNER_PRIVATE",
    "HYPERLIQUID_PRIVATE_KEY",
];
const ASTER_EXPECT_ADDRESS_ENV_DEFAULT: &str = "ASTER_SIGNER";
const ASTER_EXPECT_ADDRESS_ENV_ALIASES: &[&str] = &[
    "ASTER_SIGNER_ADDRESS",
    "ASTER_API_ADDRESS",
    "ASTER_ADDRESS",
    "ASTER_USER",
];
const HYPERLIQUID_EXPECT_ADDRESS_ENV_DEFAULT: &str = "HYPERLIQUID_AGENT";
const HYPERLIQUID_EXPECT_ADDRESS_ENV_ALIASES: &[&str] = &[
    "HYPERLIQUID_SIGNER",
    "HYPERLIQUID_SIGNER_ADDRESS",
    "HYPERLIQUID_API_ADDRESS",
    "HYPERLIQUID_AGENT_ADDRESS",
];
const ZERO_ADDRESS: [u8; 20] = [0_u8; 20];

type SignerResult<T> = Result<T, SignerError>;

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct SignerError {
    message: String,
}

impl SignerError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for SignerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for SignerError {}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct HyperliquidSignature {
    pub r: String,
    pub s: String,
    pub v: u8,
}

impl HyperliquidSignature {
    fn to_json(&self) -> String {
        format!(
            "{{\"r\":\"{}\",\"s\":\"{}\",\"v\":{}}}",
            self.r, self.s, self.v
        )
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
enum CommandMode {
    AsterEip712,
    HyperliquidL1Phantom,
    HyperliquidL1Action,
    Address,
}

#[derive(Debug, Clone, Eq, PartialEq)]
struct CliOptions {
    mode: CommandMode,
    key_env: String,
    expect_address: Option<String>,
    hyperliquid_source: Option<String>,
    hyperliquid_connection_id: Option<String>,
    hyperliquid_nonce: Option<u64>,
    hyperliquid_vault_address: Option<String>,
    hyperliquid_expires_after: Option<u64>,
}

impl CliOptions {
    fn default_aster() -> Self {
        Self {
            mode: CommandMode::AsterEip712,
            key_env: ASTER_KEY_ENV_DEFAULT.to_owned(),
            expect_address: None,
            hyperliquid_source: None,
            hyperliquid_connection_id: None,
            hyperliquid_nonce: None,
            hyperliquid_vault_address: None,
            hyperliquid_expires_after: None,
        }
    }
}

pub fn run_cli<I, S, F>(args: I, stdin: &str, env: F) -> SignerResult<String>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
    F: Fn(&str) -> Option<String>,
{
    let options = parse_cli(args)?;
    let private_key = env(&options.key_env)
        .or_else(|| default_private_key_alias(&options.mode, &options.key_env, &env))
        .ok_or_else(|| SignerError::new(private_key_env_error(&options.mode, &options.key_env)))?;
    let expected_address = options
        .expect_address
        .or_else(|| default_expected_address(&options.mode, &env));

    match options.mode {
        CommandMode::AsterEip712 => {
            let payload = stdin.trim();
            if payload.is_empty() {
                return Err(SignerError::new("Aster EIP-712 payload on stdin is empty"));
            }
            let expected_address =
                expected_address.or_else(|| extract_aster_signer_from_payload(payload).ok());
            sign_aster_eip712_payload(payload, &private_key, expected_address.as_deref())
        }
        CommandMode::HyperliquidL1Phantom => {
            let source = options
                .hyperliquid_source
                .ok_or_else(|| SignerError::new("Hyperliquid source is required"))?;
            let connection_id = options
                .hyperliquid_connection_id
                .ok_or_else(|| SignerError::new("Hyperliquid connection id is required"))?;
            let signature = sign_hyperliquid_l1_phantom_agent(
                &connection_id,
                &source,
                &private_key,
                expected_address.as_deref(),
            )?;
            Ok(signature.to_json())
        }
        CommandMode::HyperliquidL1Action => {
            let action_json = stdin.trim();
            if action_json.is_empty() {
                return Err(SignerError::new(
                    "Hyperliquid action JSON on stdin is empty",
                ));
            }
            let source = options
                .hyperliquid_source
                .ok_or_else(|| SignerError::new("Hyperliquid source is required"))?;
            let nonce = options
                .hyperliquid_nonce
                .ok_or_else(|| SignerError::new("Hyperliquid nonce is required"))?;
            let signature = sign_hyperliquid_l1_action(
                action_json,
                nonce,
                options.hyperliquid_vault_address.as_deref(),
                options.hyperliquid_expires_after,
                &source,
                &private_key,
                expected_address.as_deref(),
            )?;
            Ok(signature.to_json())
        }
        CommandMode::Address => ethereum_address_from_private_key(&private_key),
    }
}

pub fn sign_aster_eip712_payload(
    payload: &str,
    private_key_hex: &str,
    expected_address: Option<&str>,
) -> SignerResult<String> {
    validate_aster_payload(payload)?;
    let payload_signer = extract_aster_signer_from_payload(payload)?;
    if let Some(expected_address) = expected_address {
        validate_address(expected_address, "expected_address")?;
        if !payload_signer.eq_ignore_ascii_case(expected_address) {
            return Err(SignerError::new(
                "Aster payload signer does not match expected signer address",
            ));
        }
    }
    let domain_separator =
        eip712_domain_separator("AsterSignTransaction", "1", 1666, &ZERO_ADDRESS);
    let message_type_hash = keccak256(b"Message(string msg)");
    let message_hash = keccak256(payload.as_bytes());
    let struct_hash = keccak256(&concat_words(&[message_type_hash, message_hash]));
    let digest = eip712_digest(domain_separator, struct_hash);
    sign_digest_hex(&digest, private_key_hex, Some(&payload_signer))
}

pub fn sign_hyperliquid_l1_phantom_agent(
    connection_id_hex: &str,
    source: &str,
    private_key_hex: &str,
    expected_address: Option<&str>,
) -> SignerResult<HyperliquidSignature> {
    let connection_id = parse_bytes32_hex(connection_id_hex, "connection_id")?;
    let source = validate_hyperliquid_source(source)?;
    let domain_separator = eip712_domain_separator("Exchange", "1", 1337, &ZERO_ADDRESS);
    let agent_type_hash = keccak256(b"Agent(string source,bytes32 connectionId)");
    let source_hash = keccak256(source.as_bytes());
    let struct_hash = keccak256(&concat_words(&[
        agent_type_hash,
        source_hash,
        connection_id,
    ]));
    let digest = eip712_digest(domain_separator, struct_hash);
    let signature = sign_digest_parts(&digest, private_key_hex, expected_address)?;
    Ok(HyperliquidSignature {
        r: format!("0x{}", hex_lower(&signature[..32])),
        s: format!("0x{}", hex_lower(&signature[32..64])),
        v: signature[64],
    })
}

pub fn sign_hyperliquid_l1_action(
    action_json: &str,
    nonce: u64,
    vault_address: Option<&str>,
    expires_after: Option<u64>,
    source: &str,
    private_key_hex: &str,
    expected_address: Option<&str>,
) -> SignerResult<HyperliquidSignature> {
    let connection_id =
        hyperliquid_l1_action_connection_id(action_json, nonce, vault_address, expires_after)?;
    sign_hyperliquid_l1_phantom_agent(
        &format!("0x{}", hex_lower(&connection_id)),
        source,
        private_key_hex,
        expected_address,
    )
}

pub fn hyperliquid_l1_action_connection_id(
    action_json: &str,
    nonce: u64,
    vault_address: Option<&str>,
    expires_after: Option<u64>,
) -> SignerResult<[u8; 32]> {
    let action: Value = serde_json::from_str(action_json)
        .map_err(|_| SignerError::new("Hyperliquid action JSON is invalid"))?;
    let mut data = Vec::new();
    msgpack_encode_hyperliquid_action(&action, &mut data)?;
    data.extend_from_slice(&nonce.to_be_bytes());
    match vault_address {
        Some(address) => {
            data.push(1);
            data.extend_from_slice(&parse_address_bytes(address, "vault_address")?);
        }
        None => data.push(0),
    }
    if let Some(expires_after) = expires_after {
        data.push(0);
        data.extend_from_slice(&expires_after.to_be_bytes());
    }
    Ok(keccak256(&data))
}

pub fn ethereum_address_from_private_key(private_key_hex: &str) -> SignerResult<String> {
    let mut private_key_bytes = parse_private_key_hex(private_key_hex)?;
    let secret_key = SecretKey::parse(&private_key_bytes)
        .map_err(|_| SignerError::new("private key is not a valid secp256k1 secret key"))?;
    private_key_bytes.fill(0);
    let public_key = PublicKey::from_secret_key(&secret_key);
    let public_key_bytes = public_key.serialize();
    let public_key_hash = keccak256(&public_key_bytes[1..]);
    Ok(format!("0x{}", hex_lower(&public_key_hash[12..])))
}

fn parse_cli<I, S>(args: I) -> SignerResult<CliOptions>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let mut raw = args.into_iter().map(Into::into).collect::<Vec<_>>();
    let mut options = if raw.first().is_some_and(|value| {
        matches!(
            value.as_str(),
            "aster-eip712" | "hyperliquid-l1-phantom" | "hyperliquid-l1-action" | "address"
        )
    }) {
        match raw.remove(0).as_str() {
            "aster-eip712" => CliOptions::default_aster(),
            "hyperliquid-l1-phantom" => CliOptions {
                mode: CommandMode::HyperliquidL1Phantom,
                key_env: HYPERLIQUID_KEY_ENV_DEFAULT.to_owned(),
                expect_address: None,
                hyperliquid_source: None,
                hyperliquid_connection_id: None,
                hyperliquid_nonce: None,
                hyperliquid_vault_address: None,
                hyperliquid_expires_after: None,
            },
            "hyperliquid-l1-action" => CliOptions {
                mode: CommandMode::HyperliquidL1Action,
                key_env: HYPERLIQUID_KEY_ENV_DEFAULT.to_owned(),
                expect_address: None,
                hyperliquid_source: None,
                hyperliquid_connection_id: None,
                hyperliquid_nonce: None,
                hyperliquid_vault_address: None,
                hyperliquid_expires_after: None,
            },
            "address" => CliOptions {
                mode: CommandMode::Address,
                key_env: ASTER_KEY_ENV_DEFAULT.to_owned(),
                expect_address: None,
                hyperliquid_source: None,
                hyperliquid_connection_id: None,
                hyperliquid_nonce: None,
                hyperliquid_vault_address: None,
                hyperliquid_expires_after: None,
            },
            _ => unreachable!("checked command names"),
        }
    } else {
        CliOptions::default_aster()
    };

    let mut index = 0;
    while index < raw.len() {
        match raw[index].as_str() {
            "--key-env" => {
                index += 1;
                let Some(value) = raw.get(index) else {
                    return Err(SignerError::new(
                        "--key-env requires an environment variable name",
                    ));
                };
                validate_env_name(value)?;
                options.key_env = value.clone();
            }
            "--expect-address" => {
                index += 1;
                let Some(value) = raw.get(index) else {
                    return Err(SignerError::new("--expect-address requires an address"));
                };
                validate_address(value, "--expect-address")?;
                options.expect_address = Some(value.to_ascii_lowercase());
            }
            "--source" => {
                index += 1;
                let Some(value) = raw.get(index) else {
                    return Err(SignerError::new("--source requires `a` or `b`"));
                };
                options.hyperliquid_source = Some(validate_hyperliquid_source(value)?.to_owned());
            }
            "--connection-id" => {
                index += 1;
                let Some(value) = raw.get(index) else {
                    return Err(SignerError::new(
                        "--connection-id requires a bytes32 hex value",
                    ));
                };
                parse_bytes32_hex(value, "connection_id")?;
                options.hyperliquid_connection_id = Some(value.to_ascii_lowercase());
            }
            "--nonce" => {
                index += 1;
                let Some(value) = raw.get(index) else {
                    return Err(SignerError::new("--nonce requires an integer value"));
                };
                options.hyperliquid_nonce = Some(parse_u64_arg(value, "nonce")?);
            }
            "--vault-address" => {
                index += 1;
                let Some(value) = raw.get(index) else {
                    return Err(SignerError::new("--vault-address requires an address"));
                };
                validate_address(value, "--vault-address")?;
                options.hyperliquid_vault_address = Some(value.to_ascii_lowercase());
            }
            "--expires-after" => {
                index += 1;
                let Some(value) = raw.get(index) else {
                    return Err(SignerError::new(
                        "--expires-after requires an integer timestamp",
                    ));
                };
                options.hyperliquid_expires_after = Some(parse_u64_arg(value, "expires_after")?);
            }
            "--help" | "-h" => return Err(SignerError::new(usage())),
            value => {
                return Err(SignerError::new(format!(
                    "unknown arb-wallet-signer argument `{value}`\n{}",
                    usage()
                )));
            }
        }
        index += 1;
    }

    if matches!(
        options.mode,
        CommandMode::HyperliquidL1Phantom | CommandMode::HyperliquidL1Action
    ) && options.hyperliquid_source.is_none()
    {
        return Err(SignerError::new(
            "Hyperliquid signing requires --source a|b",
        ));
    }
    if matches!(options.mode, CommandMode::HyperliquidL1Phantom)
        && options.hyperliquid_connection_id.is_none()
    {
        return Err(SignerError::new(
            "hyperliquid-l1-phantom requires --connection-id 0x...",
        ));
    }
    if matches!(options.mode, CommandMode::HyperliquidL1Action)
        && options.hyperliquid_nonce.is_none()
    {
        return Err(SignerError::new(
            "hyperliquid-l1-action requires --nonce <milliseconds>",
        ));
    }

    Ok(options)
}

fn usage() -> &'static str {
    "usage: arb-wallet-signer [aster-eip712|hyperliquid-l1-phantom|hyperliquid-l1-action|address] [options]"
}

fn default_expected_address<F>(mode: &CommandMode, env: &F) -> Option<String>
where
    F: Fn(&str) -> Option<String>,
{
    match mode {
        CommandMode::AsterEip712 => env(ASTER_EXPECT_ADDRESS_ENV_DEFAULT).or_else(|| {
            ASTER_EXPECT_ADDRESS_ENV_ALIASES
                .iter()
                .find_map(|name| env(name))
        }),
        CommandMode::HyperliquidL1Phantom | CommandMode::HyperliquidL1Action => {
            env(HYPERLIQUID_EXPECT_ADDRESS_ENV_DEFAULT).or_else(|| {
                HYPERLIQUID_EXPECT_ADDRESS_ENV_ALIASES
                    .iter()
                    .find_map(|name| env(name))
            })
        }
        CommandMode::Address => None,
    }
}

fn default_private_key_alias<F>(mode: &CommandMode, key_env: &str, env: &F) -> Option<String>
where
    F: Fn(&str) -> Option<String>,
{
    match mode {
        CommandMode::AsterEip712 | CommandMode::Address if key_env == ASTER_KEY_ENV_DEFAULT => {
            first_env(ASTER_KEY_ENV_ALIASES, env)
        }
        CommandMode::HyperliquidL1Phantom | CommandMode::HyperliquidL1Action
            if key_env == HYPERLIQUID_KEY_ENV_DEFAULT =>
        {
            first_env(HYPERLIQUID_KEY_ENV_ALIASES, env)
        }
        _ => None,
    }
}

fn private_key_env_error(mode: &CommandMode, key_env: &str) -> String {
    let aliases = match mode {
        CommandMode::AsterEip712 | CommandMode::Address if key_env == ASTER_KEY_ENV_DEFAULT => {
            ASTER_KEY_ENV_ALIASES
        }
        CommandMode::HyperliquidL1Phantom | CommandMode::HyperliquidL1Action
            if key_env == HYPERLIQUID_KEY_ENV_DEFAULT =>
        {
            HYPERLIQUID_KEY_ENV_ALIASES
        }
        _ => &[],
    };
    if aliases.is_empty() {
        format!("required private key environment variable `{key_env}` is not set")
    } else {
        format!(
            "required private key environment variable `{key_env}` or one of {} is not set",
            aliases
                .iter()
                .map(|alias| format!("`{alias}`"))
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}

fn first_env<F>(names: &[&str], env: &F) -> Option<String>
where
    F: Fn(&str) -> Option<String>,
{
    names.iter().find_map(|name| env(name))
}

fn sign_digest_hex(
    digest: &[u8; 32],
    private_key_hex: &str,
    expected_address: Option<&str>,
) -> SignerResult<String> {
    let signature = sign_digest_parts(digest, private_key_hex, expected_address)?;
    Ok(format!("0x{}", hex_lower(&signature)))
}

fn sign_digest_parts(
    digest: &[u8; 32],
    private_key_hex: &str,
    expected_address: Option<&str>,
) -> SignerResult<[u8; 65]> {
    if let Some(expected) = expected_address {
        validate_private_key_matches_expected_address(private_key_hex, expected)?;
    }
    let mut private_key_bytes = parse_private_key_hex(private_key_hex)?;
    let secret_key = SecretKey::parse(&private_key_bytes)
        .map_err(|_| SignerError::new("private key is not a valid secp256k1 secret key"))?;
    private_key_bytes.fill(0);
    let message = Message::parse(digest);
    let (signature, recovery_id) = libsecp256k1::sign(&message, &secret_key);
    let compact = signature.serialize();
    let mut output = [0_u8; 65];
    output[..64].copy_from_slice(&compact);
    output[64] = recovery_id.serialize() + 27;
    Ok(output)
}

fn validate_private_key_matches_expected_address(
    private_key_hex: &str,
    expected_address: &str,
) -> SignerResult<()> {
    validate_address(expected_address, "expected_address")?;
    let actual = ethereum_address_from_private_key(private_key_hex)?;
    if actual.eq_ignore_ascii_case(expected_address) {
        Ok(())
    } else {
        Err(SignerError::new(
            "private key does not match expected signer address",
        ))
    }
}

fn eip712_domain_separator(
    name: &str,
    version: &str,
    chain_id: u64,
    verifying_contract: &[u8; 20],
) -> [u8; 32] {
    let type_hash = keccak256(
        b"EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)",
    );
    let name_hash = keccak256(name.as_bytes());
    let version_hash = keccak256(version.as_bytes());
    let chain_id_word = uint256_word(chain_id);
    let address_word = address_word(verifying_contract);
    keccak256(&concat_words(&[
        type_hash,
        name_hash,
        version_hash,
        chain_id_word,
        address_word,
    ]))
}

fn eip712_digest(domain_separator: [u8; 32], struct_hash: [u8; 32]) -> [u8; 32] {
    let mut payload = Vec::with_capacity(66);
    payload.extend_from_slice(&[0x19, 0x01]);
    payload.extend_from_slice(&domain_separator);
    payload.extend_from_slice(&struct_hash);
    keccak256(&payload)
}

fn concat_words(words: &[[u8; 32]]) -> Vec<u8> {
    let mut output = Vec::with_capacity(words.len() * 32);
    for word in words {
        output.extend_from_slice(word);
    }
    output
}

fn uint256_word(value: u64) -> [u8; 32] {
    let mut word = [0_u8; 32];
    word[24..].copy_from_slice(&value.to_be_bytes());
    word
}

fn address_word(address: &[u8; 20]) -> [u8; 32] {
    let mut word = [0_u8; 32];
    word[12..].copy_from_slice(address);
    word
}

fn keccak256(bytes: &[u8]) -> [u8; 32] {
    let mut hasher = Keccak256::new();
    hasher.update(bytes);
    hasher.finalize().into()
}

fn parse_private_key_hex(value: &str) -> SignerResult<[u8; 32]> {
    parse_bytes32_hex(value, "private_key")
}

fn parse_bytes32_hex(value: &str, field: &'static str) -> SignerResult<[u8; 32]> {
    let trimmed = value.trim();
    let hex = trimmed.strip_prefix("0x").unwrap_or(trimmed);
    if hex.len() != 64 {
        return Err(SignerError::new(format!(
            "{field} must be a 32-byte hex string"
        )));
    }
    let mut output = [0_u8; 32];
    for (index, chunk) in hex.as_bytes().chunks(2).enumerate() {
        output[index] = parse_hex_byte(chunk, field)?;
    }
    Ok(output)
}

fn parse_hex_byte(chunk: &[u8], field: &'static str) -> SignerResult<u8> {
    if chunk.len() != 2 {
        return Err(SignerError::new(format!("{field} contains malformed hex")));
    }
    let high = hex_nibble(chunk[0])
        .ok_or_else(|| SignerError::new(format!("{field} contains a non-hexadecimal character")))?;
    let low = hex_nibble(chunk[1])
        .ok_or_else(|| SignerError::new(format!("{field} contains a non-hexadecimal character")))?;
    Ok((high << 4) | low)
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

fn msgpack_encode_hyperliquid_action(action: &Value, output: &mut Vec<u8>) -> SignerResult<()> {
    let object = action
        .as_object()
        .ok_or_else(|| SignerError::new("Hyperliquid action must be a JSON object"))?;
    let action_type = json_required_string(object, "type")?;
    match action_type {
        "order" => msgpack_encode_hyperliquid_order_action(object, output),
        "cancel" => msgpack_encode_hyperliquid_cancel_action(object, output),
        "cancelByCloid" => msgpack_encode_hyperliquid_cancel_by_cloid_action(object, output),
        _ => Err(SignerError::new(
            "Hyperliquid action signer only supports order, cancel and cancelByCloid",
        )),
    }
}

fn msgpack_encode_hyperliquid_order_action(
    object: &serde_json::Map<String, Value>,
    output: &mut Vec<u8>,
) -> SignerResult<()> {
    let orders = json_required_array(object, "orders")?;
    let grouping = json_required_string(object, "grouping")?;
    let has_builder = object.get("builder").is_some_and(|value| !value.is_null());
    msgpack_write_map_len(output, if has_builder { 4 } else { 3 })?;
    msgpack_write_string(output, "type")?;
    msgpack_write_string(output, "order")?;
    msgpack_write_string(output, "orders")?;
    msgpack_write_array_len(output, orders.len())?;
    for order in orders {
        msgpack_encode_hyperliquid_order(order, output)?;
    }
    msgpack_write_string(output, "grouping")?;
    msgpack_write_string(output, grouping)?;
    if has_builder {
        msgpack_write_string(output, "builder")?;
        msgpack_encode_hyperliquid_builder(
            object
                .get("builder")
                .expect("builder presence checked above"),
            output,
        )?;
    }
    Ok(())
}

fn msgpack_encode_hyperliquid_order(order: &Value, output: &mut Vec<u8>) -> SignerResult<()> {
    let object = order
        .as_object()
        .ok_or_else(|| SignerError::new("Hyperliquid order must be a JSON object"))?;
    let has_cloid = object.get("c").is_some_and(|value| !value.is_null());
    msgpack_write_map_len(output, if has_cloid { 7 } else { 6 })?;
    msgpack_write_string(output, "a")?;
    msgpack_write_u64(output, json_required_u64(object, "a")?)?;
    msgpack_write_string(output, "b")?;
    msgpack_write_bool(output, json_required_bool(object, "b")?);
    msgpack_write_string(output, "p")?;
    msgpack_write_string(output, json_required_string(object, "p")?)?;
    msgpack_write_string(output, "s")?;
    msgpack_write_string(output, json_required_string(object, "s")?)?;
    msgpack_write_string(output, "r")?;
    msgpack_write_bool(output, json_required_bool(object, "r")?);
    msgpack_write_string(output, "t")?;
    msgpack_encode_hyperliquid_order_type(
        object
            .get("t")
            .ok_or_else(|| SignerError::new("Hyperliquid order is missing t"))?,
        output,
    )?;
    if has_cloid {
        msgpack_write_string(output, "c")?;
        msgpack_write_string(output, json_required_string(object, "c")?)?;
    }
    Ok(())
}

fn msgpack_encode_hyperliquid_order_type(
    order_type: &Value,
    output: &mut Vec<u8>,
) -> SignerResult<()> {
    let object = order_type
        .as_object()
        .ok_or_else(|| SignerError::new("Hyperliquid order type must be a JSON object"))?;
    if let Some(limit) = object.get("limit") {
        let limit = limit
            .as_object()
            .ok_or_else(|| SignerError::new("Hyperliquid limit order type must be an object"))?;
        msgpack_write_map_len(output, 1)?;
        msgpack_write_string(output, "limit")?;
        msgpack_write_map_len(output, 1)?;
        msgpack_write_string(output, "tif")?;
        msgpack_write_string(output, json_required_string(limit, "tif")?)?;
        return Ok(());
    }
    if let Some(trigger) = object.get("trigger") {
        let trigger = trigger
            .as_object()
            .ok_or_else(|| SignerError::new("Hyperliquid trigger order type must be an object"))?;
        msgpack_write_map_len(output, 1)?;
        msgpack_write_string(output, "trigger")?;
        msgpack_write_map_len(output, 3)?;
        msgpack_write_string(output, "isMarket")?;
        msgpack_write_bool(output, json_required_bool(trigger, "isMarket")?);
        msgpack_write_string(output, "triggerPx")?;
        msgpack_write_string(output, json_required_string(trigger, "triggerPx")?)?;
        msgpack_write_string(output, "tpsl")?;
        msgpack_write_string(output, json_required_string(trigger, "tpsl")?)?;
        return Ok(());
    }
    Err(SignerError::new(
        "Hyperliquid order type must contain limit or trigger",
    ))
}

fn msgpack_encode_hyperliquid_builder(builder: &Value, output: &mut Vec<u8>) -> SignerResult<()> {
    let object = builder
        .as_object()
        .ok_or_else(|| SignerError::new("Hyperliquid builder must be a JSON object"))?;
    msgpack_write_map_len(output, 2)?;
    msgpack_write_string(output, "b")?;
    msgpack_write_string(output, json_required_string(object, "b")?)?;
    msgpack_write_string(output, "f")?;
    msgpack_write_u64(output, json_required_u64(object, "f")?)
}

fn msgpack_encode_hyperliquid_cancel_action(
    object: &serde_json::Map<String, Value>,
    output: &mut Vec<u8>,
) -> SignerResult<()> {
    let cancels = json_required_array(object, "cancels")?;
    msgpack_write_map_len(output, 2)?;
    msgpack_write_string(output, "type")?;
    msgpack_write_string(output, "cancel")?;
    msgpack_write_string(output, "cancels")?;
    msgpack_write_array_len(output, cancels.len())?;
    for cancel in cancels {
        let cancel = cancel
            .as_object()
            .ok_or_else(|| SignerError::new("Hyperliquid cancel row must be an object"))?;
        msgpack_write_map_len(output, 2)?;
        msgpack_write_string(output, "a")?;
        msgpack_write_u64(output, json_required_u64(cancel, "a")?)?;
        msgpack_write_string(output, "o")?;
        msgpack_write_u64(output, json_required_u64(cancel, "o")?)?;
    }
    Ok(())
}

fn msgpack_encode_hyperliquid_cancel_by_cloid_action(
    object: &serde_json::Map<String, Value>,
    output: &mut Vec<u8>,
) -> SignerResult<()> {
    let cancels = json_required_array(object, "cancels")?;
    msgpack_write_map_len(output, 2)?;
    msgpack_write_string(output, "type")?;
    msgpack_write_string(output, "cancelByCloid")?;
    msgpack_write_string(output, "cancels")?;
    msgpack_write_array_len(output, cancels.len())?;
    for cancel in cancels {
        let cancel = cancel
            .as_object()
            .ok_or_else(|| SignerError::new("Hyperliquid cancelByCloid row must be an object"))?;
        msgpack_write_map_len(output, 2)?;
        msgpack_write_string(output, "asset")?;
        msgpack_write_u64(output, json_required_u64(cancel, "asset")?)?;
        msgpack_write_string(output, "cloid")?;
        msgpack_write_string(output, json_required_string(cancel, "cloid")?)?;
    }
    Ok(())
}

fn json_required_string<'a>(
    object: &'a serde_json::Map<String, Value>,
    field: &'static str,
) -> SignerResult<&'a str> {
    object
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| SignerError::new(format!("Hyperliquid action is missing string `{field}`")))
}

fn json_required_bool(
    object: &serde_json::Map<String, Value>,
    field: &'static str,
) -> SignerResult<bool> {
    object
        .get(field)
        .and_then(Value::as_bool)
        .ok_or_else(|| SignerError::new(format!("Hyperliquid action is missing bool `{field}`")))
}

fn json_required_u64(
    object: &serde_json::Map<String, Value>,
    field: &'static str,
) -> SignerResult<u64> {
    object
        .get(field)
        .and_then(Value::as_u64)
        .ok_or_else(|| SignerError::new(format!("Hyperliquid action is missing uint `{field}`")))
}

fn json_required_array<'a>(
    object: &'a serde_json::Map<String, Value>,
    field: &'static str,
) -> SignerResult<&'a Vec<Value>> {
    object
        .get(field)
        .and_then(Value::as_array)
        .ok_or_else(|| SignerError::new(format!("Hyperliquid action is missing array `{field}`")))
}

fn msgpack_write_map_len(output: &mut Vec<u8>, len: usize) -> SignerResult<()> {
    if len <= 15 {
        output.push(0x80 | u8::try_from(len).expect("fixmap length fits u8"));
    } else {
        return Err(SignerError::new("Hyperliquid action map is too large"));
    }
    Ok(())
}

fn msgpack_write_array_len(output: &mut Vec<u8>, len: usize) -> SignerResult<()> {
    if len <= 15 {
        output.push(0x90 | u8::try_from(len).expect("fixarray length fits u8"));
    } else if let Ok(len) = u16::try_from(len) {
        output.push(0xdc);
        output.extend_from_slice(&len.to_be_bytes());
    } else {
        return Err(SignerError::new("Hyperliquid action array is too large"));
    }
    Ok(())
}

fn msgpack_write_string(output: &mut Vec<u8>, value: &str) -> SignerResult<()> {
    if value
        .bytes()
        .any(|byte| byte == 0 || byte.is_ascii_control())
    {
        return Err(SignerError::new(
            "Hyperliquid action string contains a control byte",
        ));
    }
    let len = value.len();
    if len <= 31 {
        output.push(0xa0 | u8::try_from(len).expect("fixstr length fits u8"));
    } else if let Ok(len) = u8::try_from(len) {
        output.push(0xd9);
        output.push(len);
    } else if let Ok(len) = u16::try_from(len) {
        output.push(0xda);
        output.extend_from_slice(&len.to_be_bytes());
    } else {
        return Err(SignerError::new("Hyperliquid action string is too long"));
    }
    output.extend_from_slice(value.as_bytes());
    Ok(())
}

fn msgpack_write_bool(output: &mut Vec<u8>, value: bool) {
    output.push(if value { 0xc3 } else { 0xc2 });
}

fn msgpack_write_u64(output: &mut Vec<u8>, value: u64) -> SignerResult<()> {
    if value <= 0x7f {
        output.push(u8::try_from(value).expect("fixint fits u8"));
    } else if let Ok(value) = u8::try_from(value) {
        output.push(0xcc);
        output.push(value);
    } else if let Ok(value) = u16::try_from(value) {
        output.push(0xcd);
        output.extend_from_slice(&value.to_be_bytes());
    } else if let Ok(value) = u32::try_from(value) {
        output.push(0xce);
        output.extend_from_slice(&value.to_be_bytes());
    } else {
        output.push(0xcf);
        output.extend_from_slice(&value.to_be_bytes());
    }
    Ok(())
}

fn validate_aster_payload(payload: &str) -> SignerResult<()> {
    if payload.len() > 8192 {
        return Err(SignerError::new("Aster payload is too long"));
    }
    if payload
        .bytes()
        .any(|byte| byte == 0 || byte.is_ascii_control())
    {
        return Err(SignerError::new("Aster payload contains a control byte"));
    }
    if !payload.split('&').any(|pair| pair.starts_with("nonce=")) {
        return Err(SignerError::new("Aster payload is missing nonce"));
    }
    if !payload.split('&').any(|pair| pair.starts_with("signer=")) {
        return Err(SignerError::new("Aster payload is missing signer"));
    }
    Ok(())
}

fn extract_aster_signer_from_payload(payload: &str) -> SignerResult<String> {
    for pair in payload.split('&') {
        let Some((name, value)) = pair.split_once('=') else {
            continue;
        };
        if name == "signer" {
            let decoded = percent_decode(value)?;
            validate_address(&decoded, "signer")?;
            return Ok(decoded.to_ascii_lowercase());
        }
    }
    Err(SignerError::new("Aster payload is missing signer"))
}

fn percent_decode(value: &str) -> SignerResult<String> {
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' => {
                if index + 2 >= bytes.len() {
                    return Err(SignerError::new("percent encoding is truncated"));
                }
                output.push(parse_hex_byte(
                    &bytes[index + 1..index + 3],
                    "percent_encoding",
                )?);
                index += 3;
            }
            b'+' => {
                output.push(b' ');
                index += 1;
            }
            byte => {
                output.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8(output).map_err(|_| SignerError::new("percent decoded value is not UTF-8"))
}

fn validate_address(value: &str, field: &'static str) -> SignerResult<()> {
    let trimmed = value.trim();
    if trimmed.len() != 42 || !trimmed.starts_with("0x") {
        return Err(SignerError::new(format!(
            "{field} must be a 42-character 0x-prefixed hex address"
        )));
    }
    if !trimmed.as_bytes().iter().skip(2).all(u8::is_ascii_hexdigit) {
        return Err(SignerError::new(format!(
            "{field} must contain only hexadecimal characters"
        )));
    }
    Ok(())
}

fn parse_address_bytes(value: &str, field: &'static str) -> SignerResult<[u8; 20]> {
    validate_address(value, field)?;
    let mut output = [0_u8; 20];
    for (index, chunk) in value.as_bytes()[2..].chunks(2).enumerate() {
        output[index] = parse_hex_byte(chunk, field)?;
    }
    Ok(output)
}

fn validate_env_name(value: &str) -> SignerResult<()> {
    if value.is_empty() {
        return Err(SignerError::new(
            "environment variable name cannot be empty",
        ));
    }
    if value.len() > 128 {
        return Err(SignerError::new("environment variable name is too long"));
    }
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err(SignerError::new(
            "environment variable name must contain only ASCII letters, digits or underscore",
        ));
    }
    Ok(())
}

fn parse_u64_arg(value: &str, field: &'static str) -> SignerResult<u64> {
    value
        .parse::<u64>()
        .map_err(|_| SignerError::new(format!("{field} must be an unsigned integer")))
}

fn validate_hyperliquid_source(value: &str) -> SignerResult<&str> {
    match value {
        "a" | "b" => Ok(value),
        _ => Err(SignerError::new(
            "Hyperliquid source must be `a` for mainnet or `b` for testnet",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use libsecp256k1::{recover, RecoveryId, Signature};

    const ADDRESS: &str = "0x1a642f0e3c3af545e7acbd38b07251b3990914f1";

    #[test]
    fn derives_ethereum_address_from_private_key() {
        let key = test_key();
        assert_eq!(
            ethereum_address_from_private_key(&key).expect("address"),
            ADDRESS
        );
    }

    #[test]
    fn signs_aster_payload_and_recovers_expected_address() {
        let key = test_key();
        let payload = concat!(
            "symbol=BTCUSDT&nonce=1748310859508867&signer=",
            "0x1a642f0e3c3af545e7acbd38b07251b3990914f1"
        );
        let signature =
            sign_aster_eip712_payload(payload, &key, Some(ADDRESS)).expect("aster signature");
        assert_eq!(signature.len(), 132);
        assert!(signature.starts_with("0x"));
        assert!(
            recover_address_from_signature_hex(&aster_digest(payload), &signature)
                .expect("recover")
                .eq_ignore_ascii_case(ADDRESS)
        );
    }

    #[test]
    fn rejects_mismatched_expected_address() {
        let key = test_key();
        let payload = concat!(
            "nonce=1748310859508867&signer=",
            "0x0000000000000000000000000000000000000001"
        );
        let error = sign_aster_eip712_payload(payload, &key, None).expect_err("must reject");
        assert!(error
            .to_string()
            .contains("private key does not match expected signer address"));
    }

    #[test]
    fn rejects_payload_signer_that_differs_from_expected_address() {
        let key = test_key();
        let payload = concat!(
            "nonce=1748310859508867&signer=",
            "0x0000000000000000000000000000000000000001"
        );
        let error =
            sign_aster_eip712_payload(payload, &key, Some(ADDRESS)).expect_err("must reject");
        assert!(error
            .to_string()
            .contains("payload signer does not match expected signer"));
    }

    #[test]
    fn signs_hyperliquid_phantom_agent_as_json_signature() {
        let key = test_key();
        let signature = sign_hyperliquid_l1_phantom_agent(
            "0x1111111111111111111111111111111111111111111111111111111111111111",
            "a",
            &key,
            Some(ADDRESS),
        )
        .expect("hyperliquid signature");
        assert!(signature.r.starts_with("0x"));
        assert_eq!(signature.r.len(), 66);
        assert!(signature.s.starts_with("0x"));
        assert_eq!(signature.s.len(), 66);
        assert!(matches!(signature.v, 27 | 28));
        assert!(signature.to_json().contains("\"v\":"));
    }

    #[test]
    fn signs_hyperliquid_l1_order_action_as_json_signature() {
        let key = test_key();
        let action = concat!(
            r#"{"type":"order","orders":[{"a":0,"b":true,"p":"50000","s":"0.001","#,
            r#""r":false,"t":{"limit":{"tif":"Gtc"}},"c":"0x11111111111111111111111111111111"}],"grouping":"na"}"#
        );
        let connection_id =
            hyperliquid_l1_action_connection_id(action, 1_748_310_859_000, None, None)
                .expect("connection id");
        assert_ne!(connection_id, [0_u8; 32]);
        let signature = sign_hyperliquid_l1_action(
            action,
            1_748_310_859_000,
            None,
            None,
            "a",
            &key,
            Some(ADDRESS),
        )
        .expect("hyperliquid action signature");
        assert!(signature.r.starts_with("0x"));
        assert_eq!(signature.s.len(), 66);
        assert!(matches!(signature.v, 27 | 28));
    }

    #[test]
    fn cli_defaults_to_aster_mode_and_reads_payload_signer() {
        let key = test_key();
        let payload = concat!(
            "nonce=1748310859508867&signer=",
            "0x1a642f0e3c3af545e7acbd38b07251b3990914f1"
        );
        let output = run_cli(Vec::<String>::new(), payload, |name| {
            (name == ASTER_KEY_ENV_DEFAULT).then(|| key.clone())
        })
        .expect("cli output");
        assert_eq!(output.len(), 132);
    }

    #[test]
    fn cli_accepts_simplified_aster_key_env_alias() {
        let key = test_key();
        let payload = concat!(
            "nonce=1748310859508867&signer=",
            "0x1a642f0e3c3af545e7acbd38b07251b3990914f1"
        );
        let output = run_cli(Vec::<String>::new(), payload, |name| match name {
            "ASTER_SIGNER_PRIVATE" => Some(key.clone()),
            "ASTER_SIGNER" => Some(ADDRESS.to_owned()),
            _ => None,
        })
        .expect("cli output");
        assert_eq!(output.len(), 132);
    }

    #[test]
    fn cli_accepts_simplified_hyperliquid_key_env_alias() {
        let key = test_key();
        let args = vec![
            "hyperliquid-l1-phantom".to_owned(),
            "--source".to_owned(),
            "a".to_owned(),
            "--connection-id".to_owned(),
            "0x1111111111111111111111111111111111111111111111111111111111111111".to_owned(),
        ];
        let output = run_cli(args, "", |name| match name {
            "HYPERLIQUID_SIGNER_PRIVATE" => Some(key.clone()),
            "HYPERLIQUID_SIGNER" => Some(ADDRESS.to_owned()),
            _ => None,
        })
        .expect("cli output");
        assert!(output.contains("\"r\":\"0x"));
        assert!(output.contains("\"s\":\"0x"));
        assert!(output.contains("\"v\":"));
    }

    fn test_key() -> String {
        format!("0x{}", "01".repeat(32))
    }

    fn aster_digest(payload: &str) -> [u8; 32] {
        let domain_separator =
            eip712_domain_separator("AsterSignTransaction", "1", 1666, &ZERO_ADDRESS);
        let message_type_hash = keccak256(b"Message(string msg)");
        let message_hash = keccak256(payload.as_bytes());
        let struct_hash = keccak256(&concat_words(&[message_type_hash, message_hash]));
        eip712_digest(domain_separator, struct_hash)
    }

    fn recover_address_from_signature_hex(
        digest: &[u8; 32],
        signature: &str,
    ) -> SignerResult<String> {
        let hex = signature.strip_prefix("0x").unwrap_or(signature);
        if hex.len() != 130 {
            return Err(SignerError::new("bad signature length"));
        }
        let mut compact = [0_u8; 64];
        for (index, chunk) in hex.as_bytes()[..128].chunks(2).enumerate() {
            compact[index] = parse_hex_byte(chunk, "signature")?;
        }
        let recovery_byte = parse_hex_byte(&hex.as_bytes()[128..130], "signature")?;
        let recovery_id = RecoveryId::parse(recovery_byte - 27)
            .map_err(|_| SignerError::new("bad recovery id"))?;
        let signature = Signature::parse_standard(&compact)
            .map_err(|_| SignerError::new("bad compact signature"))?;
        let message = Message::parse(digest);
        let public_key = recover(&message, &signature, &recovery_id)
            .map_err(|_| SignerError::new("recover failed"))?;
        let public_key_bytes = public_key.serialize();
        let hash = keccak256(&public_key_bytes[1..]);
        Ok(format!("0x{}", hex_lower(&hash[12..])))
    }
}
