use crate::error::AppError;
use crate::store::AppState;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SecretScope {
    ProviderCenter,
    DshProvider,
}

impl SecretScope {
    #[cfg(target_os = "windows")]
    fn database_key(self) -> &'static str {
        match self {
            Self::ProviderCenter => "provider_center_dpapi_secrets_v1",
            Self::DshProvider => "dsh_provider_dpapi_secrets_v1",
        }
    }

    #[cfg(not(target_os = "windows"))]
    fn keyring_service(self) -> &'static str {
        match self {
            Self::ProviderCenter => "com.ccswitch.provider-center",
            Self::DshProvider => "com.ccswitch.dsh-provider",
        }
    }
}

type EncryptedSecrets = HashMap<String, String>;

fn read_json<T: for<'a> Deserialize<'a> + Default>(
    state: &AppState,
    key: &str,
) -> Result<T, AppError> {
    match state.db.get_setting(key)? {
        Some(value) => serde_json::from_str(&value)
            .map_err(|error| AppError::Database(format!("安全存储数据损坏: {error}"))),
        None => Ok(T::default()),
    }
}

fn write_json<T: Serialize>(state: &AppState, key: &str, value: &T) -> Result<(), AppError> {
    let text = serde_json::to_string(value)
        .map_err(|error| AppError::Database(format!("安全存储数据序列化失败: {error}")))?;
    state.db.set_setting(key, &text)
}

pub(crate) fn secret_hint(secret: &str) -> Option<String> {
    let chars: Vec<char> = secret.chars().collect();
    (chars.len() >= 4).then(|| format!("…{}", chars[chars.len() - 4..].iter().collect::<String>()))
}

#[cfg(target_os = "windows")]
pub(crate) fn protect_secret(secret: &str) -> Result<String, AppError> {
    use std::ffi::c_void;
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    let bytes = secret.as_bytes();
    let input = CRYPT_INTEGER_BLOB {
        cbData: bytes.len() as u32,
        pbData: bytes.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: null_mut(),
    };
    let ok = unsafe {
        CryptProtectData(
            &input,
            null(),
            null(),
            null_mut(),
            null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    };
    if ok == 0 {
        return Err(AppError::Message(
            "无法使用 Windows 安全存储保护 API Key".to_string(),
        ));
    }
    let protected =
        unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
    unsafe { LocalFree(output.pbData as *mut c_void) };
    Ok(BASE64.encode(protected))
}

#[cfg(target_os = "windows")]
pub(crate) fn unprotect_secret(blob: &str) -> Result<String, AppError> {
    use std::ffi::c_void;
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    let mut bytes = BASE64
        .decode(blob)
        .map_err(|_| AppError::Message("已保存的 API Key 数据无效".to_string()))?;
    let input = CRYPT_INTEGER_BLOB {
        cbData: bytes.len() as u32,
        pbData: bytes.as_mut_ptr(),
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: null_mut(),
    };
    let ok = unsafe {
        CryptUnprotectData(
            &input,
            null_mut(),
            null(),
            null_mut(),
            null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    };
    if ok == 0 {
        return Err(AppError::Message(
            "无法从 Windows 安全存储读取 API Key".to_string(),
        ));
    }
    let secret =
        unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
    unsafe { LocalFree(output.pbData as *mut c_void) };
    String::from_utf8(secret)
        .map_err(|_| AppError::Message("已保存的 API Key 不是有效文本".to_string()))
}

#[cfg(target_os = "windows")]
pub(crate) fn save(
    state: &AppState,
    scope: SecretScope,
    id: &str,
    secret: &str,
) -> Result<(), AppError> {
    let mut secrets: EncryptedSecrets = read_json(state, scope.database_key())?;
    secrets.insert(id.to_string(), protect_secret(secret)?);
    write_json(state, scope.database_key(), &secrets)
}

#[cfg(target_os = "windows")]
pub(crate) fn remove(state: &AppState, scope: SecretScope, id: &str) -> Result<(), AppError> {
    let mut secrets: EncryptedSecrets = read_json(state, scope.database_key())?;
    secrets.remove(id);
    write_json(state, scope.database_key(), &secrets)
}

#[cfg(target_os = "windows")]
pub(crate) fn metadata(
    state: &AppState,
    scope: SecretScope,
    id: &str,
) -> Result<(bool, Option<String>), AppError> {
    let secrets: EncryptedSecrets = read_json(state, scope.database_key())?;
    let Some(blob) = secrets.get(id) else {
        return Ok((false, None));
    };
    let secret = unprotect_secret(blob)?;
    Ok((true, secret_hint(&secret)))
}

#[cfg(target_os = "windows")]
pub(crate) fn get(state: &AppState, scope: SecretScope, id: &str) -> Result<String, AppError> {
    let secrets: EncryptedSecrets = read_json(state, scope.database_key())?;
    let blob = secrets
        .get(id)
        .ok_or_else(|| AppError::Message("该模型服务没有可用的 API Key".to_string()))?;
    unprotect_secret(blob)
}

#[cfg(not(target_os = "windows"))]
fn secure_entry(scope: SecretScope, id: &str) -> Result<keyring::Entry, AppError> {
    keyring::Entry::new(scope.keyring_service(), id)
        .map_err(|error| AppError::Message(format!("无法访问系统安全存储: {error}")))
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn save(
    _: &AppState,
    scope: SecretScope,
    id: &str,
    secret: &str,
) -> Result<(), AppError> {
    secure_entry(scope, id)?
        .set_password(secret)
        .map_err(|error| AppError::Message(format!("无法写入系统安全存储: {error}")))
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn remove(_: &AppState, scope: SecretScope, id: &str) -> Result<(), AppError> {
    match secure_entry(scope, id)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(AppError::Message(format!("无法清除系统安全存储: {error}"))),
    }
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn metadata(
    _: &AppState,
    scope: SecretScope,
    id: &str,
) -> Result<(bool, Option<String>), AppError> {
    match secure_entry(scope, id)?.get_password() {
        Ok(secret) => Ok((true, secret_hint(&secret))),
        Err(keyring::Error::NoEntry) => Ok((false, None)),
        Err(error) => Err(AppError::Message(format!("无法读取系统安全存储: {error}"))),
    }
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn get(_: &AppState, scope: SecretScope, id: &str) -> Result<String, AppError> {
    match secure_entry(scope, id)?.get_password() {
        Ok(secret) => Ok(secret),
        Err(keyring::Error::NoEntry) => Err(AppError::Message(
            "该模型服务没有可用的 API Key".to_string(),
        )),
        Err(error) => Err(AppError::Message(format!("无法读取系统安全存储: {error}"))),
    }
}

pub(crate) fn restore(
    state: &AppState,
    scope: SecretScope,
    id: &str,
    previous: Option<&str>,
) -> Result<(), AppError> {
    match previous {
        Some(secret) => save(state, scope, id, secret),
        None => remove(state, scope, id),
    }
}
