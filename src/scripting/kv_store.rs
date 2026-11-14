/// Abstract key-value store wrapper (Sled or Redis).
pub enum KvStore {
    Sled(sled::Db),
    #[cfg(feature = "database")]
    Redis(redis::Client),
}

impl KvStore {
    /// Retrieves a value by key.
    pub fn get(&self, key: &str) -> Result<Option<String>, String> {
        match self {
            KvStore::Sled(db) => match db.get(key) {
                Ok(Some(val)) => {
                    let s = String::from_utf8_lossy(&val).to_string();
                    Ok(Some(s))
                }
                Ok(None) => Ok(None),
                Err(e) => Err(format!("Sled get error: {}", e)),
            },
            #[cfg(feature = "database")]
            KvStore::Redis(client) => {
                let mut con = client
                    .get_connection()
                    .map_err(|e| format!("Redis conn error: {}", e))?;
                let val: Option<String> = redis::cmd("GET")
                    .arg(key)
                    .query(&mut con)
                    .map_err(|e| format!("Redis get error: {}", e))?;
                Ok(val)
            }
        }
    }

    /// Stores a value by key.
    pub fn set(&self, key: &str, value: &str) -> Result<(), String> {
        match self {
            KvStore::Sled(db) => {
                db.insert(key, value.as_bytes())
                    .map_err(|e| format!("Sled set error: {}", e))?;
                let _ = db.flush();
                Ok(())
            }
            #[cfg(feature = "database")]
            KvStore::Redis(client) => {
                let mut con = client
                    .get_connection()
                    .map_err(|e| format!("Redis conn error: {}", e))?;
                let _: () = redis::cmd("SET")
                    .arg(key)
                    .arg(value)
                    .query(&mut con)
                    .map_err(|e| format!("Redis set error: {}", e))?;
                Ok(())
            }
        }
    }

    /// Dumps all key-value pairs as a JSON string.
    pub fn dump(&self) -> Result<String, String> {
        match self {
            KvStore::Sled(db) => {
                let mut map = std::collections::HashMap::new();
                for (k, v) in db.iter().flatten() {
                    let key_str = String::from_utf8_lossy(&k).to_string();
                    let val_str = String::from_utf8_lossy(&v).to_string();
                    map.insert(key_str, val_str);
                }
                serde_json::to_string_pretty(&map).map_err(|e| format!("JSON error: {}", e))
            }
            #[cfg(feature = "database")]
            KvStore::Redis(client) => {
                let mut con = client
                    .get_connection()
                    .map_err(|e| format!("Redis conn error: {}", e))?;
                let keys: Vec<String> = redis::cmd("KEYS")
                    .arg("*")
                    .query(&mut con)
                    .map_err(|e| format!("Redis keys error: {}", e))?;
                let mut map = std::collections::HashMap::new();
                for k in keys {
                    let val: Option<String> = redis::cmd("GET")
                        .arg(&k)
                        .query(&mut con)
                        .map_err(|e| format!("Redis get error: {}", e))?;
                    if let Some(v) = val {
                        map.insert(k, v);
                    }
                }
                serde_json::to_string_pretty(&map).map_err(|e| format!("JSON error: {}", e))
            }
        }
    }
}
