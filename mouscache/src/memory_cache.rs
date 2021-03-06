use std::time::{Instant, Duration};
use std::collections::hash_map::HashMap;
use std::collections::hash_set::HashSet;
use crate::Result;
use crate::Cacheable;
use crate::CacheAccess;
use crate::CacheFunc;
use parking_lot::RwLock;
use std::sync::Arc;
use std::str::FromStr;

struct Expiration {
    insertion_time: Instant,
    ttl: Duration,
}

impl Expiration {
    pub fn new(ttl: usize) -> Self {
        Expiration {
            insertion_time: Instant::now(),
            ttl: Duration::from_secs(ttl as u64),
        }
    }

    pub fn is_expired(&self) -> bool {
        let time_since_insertion = Instant::now().duration_since(self.insertion_time);
        time_since_insertion >= self.ttl
    }
}

type MemCacheable = (Box<dyn Cacheable>, Option<Expiration>);

struct Inner {
    pub obj_cache: RwLock<HashMap<String, MemCacheable>>,
    pub hashsets: RwLock<HashMap<String, RwLock<HashMap<String, String>>>>,
    pub sets: RwLock<HashMap<String, RwLock<HashSet<String>>>>,
}

impl Inner {
    pub fn new() -> Self {
        Inner {
            obj_cache: RwLock::new(HashMap::new()),
            hashsets: RwLock::new(HashMap::new()),
            sets: RwLock::new(HashMap::new()),
        }
    }

    fn hash_exists(&self, key: &str) -> bool {
        self.hashsets.read().get(key).is_some()
    }

    pub fn ensure_hash_exists(&self, key: &str) -> Result<()> {
        if self.hash_exists(key) {
            return Ok(());
        } else {
            let mut writer = self.hashsets.write();
            if let Some(_) = writer.insert(key.to_string(), RwLock::new(HashMap::new())) {
                return Err(crate::CacheError::Other("Unable to insert a new hashmap".to_string()));
            }
        }
        Ok(())
    }

    fn set_exists(&self, key: &str) -> bool {
        self.sets.read().get(key).is_some()
    }

    pub fn ensure_set_exists(&self, key: &str) -> Result<()> {
        if self.set_exists(key) {
            return Ok(());
        } else {
            let mut writer = self.sets.write();
            if let Some(_) = writer.insert(key.to_string(), RwLock::new(HashSet::new())) {
                return Err(crate::CacheError::Other("Unable to insert a new hashset".to_string()));
            }
        }
        Ok(())
    }
}

pub struct MemoryCache {
    inner: Arc<Inner>
}

impl Clone for MemoryCache {
    fn clone(&self) -> Self {
        MemoryCache {
            inner: self.inner.clone(),
        }
    }
}

impl MemoryCache {
    pub fn new() -> MemoryCache {
        MemoryCache {
            inner: Arc::new(Inner::new())
        }
    }
}

impl CacheAccess for MemoryCache {
    fn insert<K: ToString, O: Cacheable + Clone + 'static>(&self, key: K, obj: O) -> Result<()> {
        let exp = obj.expires_after();
        self.insert_with(key, obj, exp)
    }

    fn insert_with<K: ToString, O: Cacheable + Clone + 'static>(&self, key: K, obj: O, expires_after: Option<usize>) -> Result<()> {
        let tkey = gen_key::<K, O>(key);

        let exp = expires_after.map(|ttl| { Expiration::new(ttl) });

        self.inner.obj_cache.write().insert(tkey, (Box::new(obj), exp));
        Ok(())
    }

    fn get<K: ToString, O: Cacheable + Clone + 'static>(&self, key: K) -> Result<Option<O>> {
        let tkey = gen_key::<K, O>(key);

        let mut delete_entry = false;

        {
            let cache = self.inner.obj_cache.read();
            if let Some(&(ref obj, ref exp)) = cache.get(&tkey) {
                if let &Some(ref exp) = exp {
                    if exp.is_expired() {
                        delete_entry = true;
                    }
                }

                if !delete_entry {
                    let struct_obj: O = match obj.as_any().downcast_ref::<O>() {
                        Some(struct_obj) => struct_obj.clone(),
                        None => panic!("Invalid type in mouscache")
                    };

                    return Ok(Some(struct_obj));
                }
            }
        }

        if delete_entry {
            let mut cache = self.inner.obj_cache.write();
            cache.remove(&tkey);
        }

        Ok(None)
    }

    fn contains_key<K: ToString, O: Cacheable + Clone + 'static>(&self, key: K) -> Result<bool> {
        let cache = self.inner.obj_cache.read();
        let tkey = gen_key::<K, O>(key);
        Ok(cache.contains_key(&tkey))
    }

    fn remove<K: ToString, O: Cacheable>(&self, key: K) -> Result<()> {
        let tkey = gen_key::<K, O>(key);
        self.inner.obj_cache.write().remove(&tkey);
        Ok(())
    }
}

fn gen_key<K: ToString, O: Cacheable>(key: K) -> String {
    format!("{}:{}", O::model_name(), key.to_string())
}

impl CacheFunc for MemoryCache {
    fn hash_delete(&self, key: &str, fields: &[&str]) -> Result<bool> {
        let map = self.inner.hashsets.read();
        if let Some(hash) = map.get(key) {
            for f in fields {
                hash.write().remove(&f.to_string());
            }
        }
        Ok(true)
    }

    fn hash_exists(&self, key: &str, field: &str) -> Result<bool> {
        let map = self.inner.hashsets.read();
        if let Some(hash) = map.get(key) {
            Ok(hash.read().contains_key(field))
        } else {
            Ok(false)
        }
    }

    fn hash_get<T: FromStr>(&self, key: &str, field: &str) -> Result<Option<T>> {
        let map = self.inner.hashsets.read();
        if let Some(hash) = map.get(key) {
            if let Some(val) = hash.read().get(field) {
                return T::from_str(val).map(|t| Some(t)).map_err(|_| crate::CacheError::Other("Unable to parse value into desired type".to_string()));
            }
        }
        Ok(None)
    }

    fn hash_get_all<T: Cacheable + Clone + 'static>(&self, key: &str) -> Result<Option<T>> {
        self.get::<&str, T>(key)
    }

    fn hash_keys(&self, key: &str) -> Result<Vec<String>> {
        let map = self.inner.hashsets.read();
        if let Some(hash) = map.get(key) {
            let res = hash.read().keys().map(|k| k.clone()).collect();
            return Ok(res);
        }
        Ok(vec!())
    }

    fn hash_len(&self, key: &str) -> Result<usize> {
        let map = self.inner.hashsets.read();
        if let Some(hash) = map.get(key) {
            return Ok(hash.read().len());
        }
        Ok(0)
    }

    fn hash_multiple_get(&self, key: &str, fields: &[&str]) -> Result<Vec<Option<String>>> {
        let mut vec = Vec::new();
        let map = self.inner.hashsets.read();
        if let Some(hash) = map.get(key) {
            let reader = hash.read();
            for f in fields {
                vec.push(reader.get(f.clone()).map(|s| s.clone()));
            }
        }

        Ok(vec)
    }

    fn hash_multiple_set<V: ToString>(&self, key: &str, fv_pairs: &[(&str, V)]) -> Result<bool> {
        self.inner.ensure_hash_exists(key)?;
        let map = self.inner.hashsets.read();
        if let Some(hash) = map.get(key) {
            let mut writer = hash.write();
            for pair in fv_pairs {
                writer.insert(pair.0.to_string(), pair.1.to_string());
            }
            Ok(true)
        } else {
            Err(crate::CacheError::Other("Unable to retrive hash from key".to_string()))
        }
    }

    fn hash_set<V: ToString>(&self, key: &str, field: &str, value: V) -> Result<bool> {
        self.inner.ensure_hash_exists(key)?;
        let map = self.inner.hashsets.read();
        if let Some(hash) = map.get(key) {
            hash.write().insert(field.to_string(), value.to_string());
            Ok(true)
        } else {
            Err(crate::CacheError::Other("Unable to retrive hash from key".to_string()))
        }
    }

    fn hash_set_all<T: Cacheable + Clone + 'static>(&self, key: &str, cacheable: T) -> Result<bool> {
        self.insert(key, cacheable).map(|_| true)
    }

    fn hash_set_if_not_exists<V: ToString>(&self, key: &str, field: &str, value: V) -> Result<bool> {
        self.inner.ensure_hash_exists(key)?;
        let map = self.inner.hashsets.read();
        if let Some(hash) = map.get(key) {
            {
                if hash.read().contains_key(field) {
                    return Ok(false);
                }
            }
            {
                hash.write().insert(field.to_string(), value.to_string());
                Ok(true)
            }
        } else {
            Err(crate::CacheError::Other("Unable to retrive hash from key".to_string()))
        }
    }

    fn hash_values(&self, key: &str) -> Result<Vec<String>> {
        let map = self.inner.hashsets.read();
        let vec = if let Some(hash) = map.get(key) {
            hash.read().values().map(|s| s.clone()).collect()
        } else {
            Vec::new()
        };

        Ok(vec)
    }

    fn set_add<V: ToString>(&self, key: &str, members: &[V]) -> Result<bool> {
        self.inner.ensure_set_exists(key)?;
        let sets = self.inner.sets.read();
        if let Some(set) = sets.get(key) {
            let mut writer = set.write();
            for m in members {
                writer.insert(m.to_string());
            }
            Ok(true)
        } else {
            Err(crate::CacheError::Other("Unable to retrive set from key".to_string()))
        }
    }

    fn set_card(&self, key: &str) -> Result<u64> {
        let sets = self.inner.sets.read();
        if let Some(set) = sets.get(key) {
            return Ok(set.read().len() as u64);
        }
        Ok(0)
    }

    fn set_diff(&self, keys: &[&str]) -> Result<Vec<String>> {
        let sets = self.inner.sets.read();
        let mut siter = keys.iter().filter_map(|key| {
            sets.get(key.clone())
        });

        if let Some(set) = siter.next() {
            let res = siter.fold(set.read().clone(), |diff_set, current_set_lock| {
                diff_set.difference(&current_set_lock.read()).map(|sref| sref.clone()).collect()
            }).iter().map(|sref| sref.clone()).collect::<Vec<_>>();

            Ok(res)
        } else {
            Ok(vec![])
        }
    }

    fn set_diffstore(&self, diff_name: &str, keys: &[&str]) -> Result<u64> {
        self.inner.ensure_set_exists(diff_name)?;
        let sets = self.inner.sets.read();
        let mut siter = keys.iter().filter_map(|key| {
            sets.get(key.clone())
        });

        if let Some(set) = siter.next() {
            let res = siter.fold(set.read().clone(), |diff_set, current_set_lock| {
                diff_set.difference(&current_set_lock.read()).map(|sref| sref.clone()).collect()
            }).iter().map(|sref| sref.clone()).collect::<Vec<_>>();

            if let Ok(true) = self.set_add(diff_name, &res) {
                Ok(res.len() as u64)
            } else {
                Ok(0)
            }
        } else {
            Ok(0)
        }
    }

    fn set_inter(&self, keys: &[&str]) -> Result<Vec<String>> {
        let sets = self.inner.sets.read();
        let mut siter = keys.iter().filter_map(|key| {
            sets.get(key.clone())
        });

        if let Some(set) = siter.next() {
            let res = siter.fold(set.read().clone(), |inter_set, current_set_lock| {
                inter_set.intersection(&current_set_lock.read()).map(|sref| sref.clone()).collect()
            }).iter().map(|sref| sref.clone()).collect::<Vec<_>>();

            Ok(res)
        } else {
            Ok(vec![])
        }
    }

    fn set_interstore(&self, inter_name: &str, keys: &[&str]) -> Result<u64> {
        self.inner.ensure_set_exists(inter_name)?;
        let sets = self.inner.sets.read();
        let mut siter = keys.iter().filter_map(|key| {
            sets.get(key.clone())
        });

        if let Some(set) = siter.next() {
            let res = siter.fold(set.read().clone(), |inter_set, current_set_lock| {
                inter_set.intersection(&current_set_lock.read()).map(|sref| sref.clone()).collect()
            }).iter().map(|sref| sref.clone()).collect::<Vec<_>>();

            if let Ok(true) = self.set_add(inter_name, &res) {
                Ok(res.len() as u64)
            } else {
                Ok(0)
            }
        } else {
            Ok(0)
        }
    }

    fn set_ismember<V: ToString>(&self, key: &str, member: V) -> Result<bool> {
        let sets = self.inner.sets.read();
        if let Some(set) = sets.get(key) {
            Ok(set.read().contains(&member.to_string()))
        } else {
            Ok(false)
        }
    }

    fn set_members(&self, key: &str) -> Result<Vec<String>> {
        let sets = self.inner.sets.read();
        if let Some(set) = sets.get(key) {
            Ok(set.read().iter().map(|ref_str| ref_str.clone()).collect::<Vec<String>>())
        } else {
            Ok(vec![])
        }
    }

    fn set_move<V: ToString>(&self, key1: &str, key2: &str, member: V) -> Result<bool> {
        let set_member = member.to_string();
        let sets = self.inner.sets.read();
        if let Some(set) = sets.get(key1) {
            let inserted = {
                if set.read().contains(&set_member) {
                    sets[key2].write().insert(set_member.clone());
                    true
                } else { false }
            };
            if inserted {
                set.write().remove(&set_member);
                Ok(true)
            } else {
                Ok(false)
            }
        } else {
            Ok(false)
        }
    }

    fn set_rem<V: ToString>(&self, key: &str, member: V) -> Result<bool> {
        let sets = self.inner.sets.read();
        if let Some(set) = sets.get(key) {
            Ok(set.write().remove(&member.to_string()))
        } else {
            Ok(false)
        }
    }

    fn set_union(&self, keys: &[&str]) -> Result<Vec<String>> {
        let sets = self.inner.sets.read();
        let mut siter = keys.iter().filter_map(|key| {
            sets.get(key.clone())
        });

        if let Some(set) = siter.next() {
            let res = siter.fold(set.read().clone(), |union_set, current_set_lock| {
                union_set.union(&current_set_lock.read()).map(|sref| sref.clone()).collect()
            }).iter().map(|sref| sref.clone()).collect::<Vec<_>>();

            Ok(res)
        } else {
            Ok(vec![])
        }
    }

    fn set_unionstore(&self, union_name: &str, keys: &[&str]) -> Result<u64> {
        self.inner.ensure_set_exists(union_name)?;
        let sets = self.inner.sets.read();
        let mut siter = keys.iter().filter_map(|key| {
            sets.get(key.clone())
        });

        if let Some(set) = siter.next() {
            let res = siter.fold(set.read().clone(), |union_set, current_set_lock| {
                union_set.union(&current_set_lock.read()).map(|sref| sref.clone()).collect()
            }).iter().map(|sref| sref.clone()).collect::<Vec<_>>();

            if let Ok(true) = self.set_add(union_name, &res) {
                Ok(res.len() as u64)
            } else {
                Ok(0)
            }
        } else {
            Ok(0)
        }
    }
}
