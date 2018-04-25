use std::net;
use std::mem::discriminant;
use std::collections::hash_map::HashMap;
use Cache;
use Cache::Redis;
use Result;
use CacheError;
use Cacheable;
use CacheAccess;
use redis;
use redis::Commands;
use dns_lookup::lookup_host;

use r2d2::Pool;
use r2d2_redis::RedisConnectionManager;

#[allow(dead_code)]
pub struct RedisCache {
    connection_pool: Pool<RedisConnectionManager>,
}

impl RedisCache {
    pub fn new(host: &str, password: Option<&str>) -> Result<Cache> {
        let host_vec: Vec<&str> = host.split(":").collect();

        let ips: Vec<net::IpAddr> = match lookup_host(host_vec[0]) {
            Ok(hosts) => hosts,
            Err(e) => return Err(CacheError::Other(e.to_string())),
        };

        if let Some((_, ip_v4)) = ips.iter()
            .enumerate()
            .find(|&(_index, ip)| {
                discriminant(ip) == discriminant(&net::IpAddr::V4(net::Ipv4Addr::new(0,0,0,0)))
            }) {

            let ip_host = if host_vec.len() > 1 {
                format!("{}:{}", ip_v4.to_string(), host_vec[1])
            } else {
                ip_v4.to_string()
            };

            let url = match password {
                Some(p) => format!("redis://:{}@{}", p, ip_host),
                None => format!("redis://{}", ip_host),
            };

            let manager = match RedisConnectionManager::new(url.as_str()) {
                Ok(m) => m,
                Err(e) => return Err(CacheError::Other(e.to_string())),
            };

            let connection_pool = match Pool::builder().build(manager) {
                Ok(cp) => cp,
                Err(e) => return Err(CacheError::Other(e.to_string())),
            };

            return Ok(Redis(RedisCache {
                connection_pool,
            }));
        }

        Err(CacheError::Other(format!("Could'n find any valid IP for host {} ", host)))
    }
}

impl CacheAccess for RedisCache {
    fn insert<K: ToString, O: Cacheable + 'static>(&mut self, key: K, obj: O) -> Result<()> {
        let connection = match self.connection_pool.get() {
            Ok(con) => con,
            Err(e) => return Err(CacheError::ConnectionError(e.to_string())),
        };

        let redis_key = redis_key_create::<K, O>(key);
        let data = obj.to_redis_obj();
        if let Some(ttl) = obj.expires_after() {
            redis_hash_set_multiple_with_expire(&connection, redis_key, &data, ttl)
        } else {
            redis_hash_set_multiple(&connection, redis_key, &data)
        }
    }

    fn get<K: ToString, O: Cacheable + 'static>(&mut self, key: K) -> Result<Option<O>> {
        let connection = match self.connection_pool.get() {
            Ok(con) => con,
            Err(e) => return Err(CacheError::ConnectionError(e.to_string())),
        };

        let redis_key = redis_key_create::<K, O>(key);
        if let Ok(val) = redis_hash_get_all(&connection, redis_key) {
            if let Ok(c) = O::from_redis_obj(val) {
                Ok(Some(c))
            } else {
                Ok(None)
            }
        } else {
            Ok(None)
        }
    }

    fn remove<K: ToString, O: Cacheable>(&mut self, key: K) -> Result<()> {
        let connection = match self.connection_pool.get() {
            Ok(con) => con,
            Err(e) => return Err(CacheError::ConnectionError(e.to_string())),
        };

        let redis_key = redis_key_create::<K, O>(key);
        redis_delete(&connection, redis_key)
    }
}

fn redis_key_create<K: ToString, O: Cacheable>(key: K) -> String {
    let mut redis_key = String::from(O::model_name());
    redis_key.push_str(":");
    redis_key.push_str(key.to_string().as_str());
    redis_key
}

fn redis_hash_set_multiple_with_expire<F: redis::ToRedisArgs, V: redis::ToRedisArgs>(con: &redis::Connection, key: String, v: &[(F, V)], ttl_sec: usize) -> Result<()> {
    if let Ok(_) = redis_hash_set_multiple(con, key.clone(), v) {
        match con.expire(key, ttl_sec) {
            Ok(v) => Ok(v),
            Err(e) => Err(CacheError::InsertionError(e.to_string())),
        }
    } else {
        Err(CacheError::InsertionError(String::new()))
    }
}

fn redis_hash_set_multiple<F: redis::ToRedisArgs, V: redis::ToRedisArgs>(con: &redis::Connection, key: String, v: &[(F, V)]) -> Result<()> {
    match con.hset_multiple::<String, F, V, ()>(key, v) {
        Ok(_) => Ok(()),
        Err(_) => Err(CacheError::InsertionError(String::new())),
    }
}

fn redis_hash_get_all(con: &redis::Connection, key: String) -> Result<HashMap<String, String>> {
    match con.hgetall::<String, HashMap<String, String>>(key) {
        Ok(v) => Ok(v),
        Err(e) => Err(CacheError::Other(e.to_string())),
    }
}

fn redis_delete(con: &redis::Connection, key: String) -> Result<()> {
    match con.del::<String, ()>(key) {
        Ok(_) => Ok(()),
        Err(_) => Err(CacheError::DeletionError(String::new())),
    }
}