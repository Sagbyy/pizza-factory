use std::collections::HashMap;
use std::fs::{remove_file, write};
use std::sync::{OnceLock, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

const PATH: &str = "db/orders.json";

pub struct StoreGuard {
    pub delete_on_drop: bool,
}

impl Drop for StoreGuard {
    fn drop(&mut self) {
        if self.delete_on_drop {
            let _ = remove_file(PATH);
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub enum OrderStatus {
    Sending,
    Receipt,
    Declined(String),
    Delivered,
    Failed(String),
    Error(String),
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Order {
    pub id: u128,
    pub server_id: Option<String>,
    pub recipe_name: String,
    pub status: OrderStatus,
    pub timestamp_ms: u64,
}

impl Order {
    pub fn elapsed_ms(&self) -> u128 {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        now_ms.saturating_sub(self.timestamp_ms as u128)
    }
}

pub static ORDERS: OnceLock<RwLock<HashMap<u128, Order>>> = OnceLock::new();

pub fn init_store() -> StoreGuard {
    let _ = std::fs::create_dir_all("db");

    let map = std::fs::read_to_string(PATH)
        .ok()
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default();

    ORDERS.set(RwLock::new(map)).ok();
    StoreGuard {
        delete_on_drop: false,
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn save_to_file(orders: &HashMap<u128, Order>) {
    if let Ok(json) = serde_json::to_string(orders) {
        let _ = write(PATH, json);
    }
}

pub fn add_order(order: Order) {
    let mut orders = ORDERS.get().unwrap().write().unwrap();
    orders.insert(order.id, order);
    save_to_file(&orders);
}

pub fn update_order_server_id(id: u128, server_id: &str) {
    let mut orders = ORDERS.get().unwrap().write().unwrap();
    if let Some(order) = orders.get_mut(&id) {
        order.server_id = Some(server_id.to_string());
        save_to_file(&orders);
    }
}

pub fn update_order_status(id: u128, status: OrderStatus) {
    let mut orders = ORDERS.get().unwrap().write().unwrap();
    if let Some(order) = orders.get_mut(&id) {
        order.status = status;
        save_to_file(&orders);
    }
}

pub fn get_orders() -> Vec<Order> {
    std::fs::read_to_string(PATH)
        .ok()
        .and_then(|json| serde_json::from_str::<HashMap<u128, Order>>(&json).ok())
        .map(|map| map.into_values().collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static TEST_MUTEX: Mutex<()> = Mutex::new(());

    fn clear_orders() {
        let _ = remove_file(PATH);
        if let Some(orders) = ORDERS.get() {
            orders.write().unwrap().clear();
        } else {
            ORDERS.set(RwLock::new(HashMap::new())).ok();
        }
    }

    fn make_order(id: u128, recipe: &str) -> Order {
        Order {
            id,
            server_id: None,
            recipe_name: recipe.to_string(),
            status: OrderStatus::Sending,
            timestamp_ms: now_ms(),
        }
    }

    #[test]
    fn test_add_order() {
        let _lock = TEST_MUTEX.lock().unwrap();
        clear_orders();

        add_order(make_order(1, "margherita"));
        assert_eq!(get_orders().len(), 1);
    }

    #[test]
    fn test_add_multiple_orders() {
        let _lock = TEST_MUTEX.lock().unwrap();
        clear_orders();

        add_order(make_order(1, "margherita"));
        add_order(make_order(2, "pepperoni"));
        assert_eq!(get_orders().len(), 2);
    }

    #[test]
    fn test_add_order_overwrites_same_id() {
        let _lock = TEST_MUTEX.lock().unwrap();
        clear_orders();

        add_order(make_order(1, "margherita"));
        add_order(make_order(1, "pepperoni"));

        let orders = get_orders();
        assert_eq!(orders.len(), 1);
        assert_eq!(orders[0].recipe_name, "pepperoni");
    }

    #[test]
    fn test_update_order_server_id() {
        let _lock = TEST_MUTEX.lock().unwrap();
        clear_orders();

        add_order(make_order(1, "margherita"));
        update_order_server_id(1, "server-uuid-123");

        let orders = get_orders();
        assert_eq!(orders[0].server_id.as_deref(), Some("server-uuid-123"));
    }

    #[test]
    fn test_update_order_status() {
        let _lock = TEST_MUTEX.lock().unwrap();
        clear_orders();

        add_order(make_order(1, "margherita"));
        update_order_status(1, OrderStatus::Receipt);

        let orders = get_orders();
        assert!(matches!(orders[0].status, OrderStatus::Receipt));
    }

    #[test]
    fn test_update_order_status_failed() {
        let _lock = TEST_MUTEX.lock().unwrap();
        clear_orders();

        add_order(make_order(1, "margherita"));
        update_order_status(1, OrderStatus::Failed("timeout".to_string()));

        let orders = get_orders();
        assert!(matches!(&orders[0].status, OrderStatus::Failed(msg) if msg == "timeout"));
    }

    #[test]
    fn test_update_nonexistent_order_does_not_panic() {
        let _lock = TEST_MUTEX.lock().unwrap();
        clear_orders();

        update_order_status(999, OrderStatus::Delivered);
        update_order_server_id(999, "some-id");
    }

    #[test]
    fn test_file_created_on_add() {
        let _lock = TEST_MUTEX.lock().unwrap();
        clear_orders();

        add_order(make_order(1, "margherita"));
        assert!(std::path::Path::new(PATH).exists());
    }

    #[test]
    fn test_file_deleted_on_drop() {
        let _lock = TEST_MUTEX.lock().unwrap();
        clear_orders();
        add_order(make_order(1, "margherita"));
        assert!(std::path::Path::new(PATH).exists());
        drop(StoreGuard {
            delete_on_drop: true,
        });
        assert!(!std::path::Path::new(PATH).exists());
    }
}
