use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};

#[derive(Clone)]
pub enum OrderStatus {
    Sending,
    Receipt,
    Declined(String),
    Delivered,
    Failed(String),
    Error(String),
}

#[derive(Clone)]
pub struct Order {
    pub id: u128,
    pub server_id: Option<String>,
    pub recipe_name: String,
    pub status: OrderStatus,
    pub timestamp: std::time::SystemTime,
}

pub static ORDERS: OnceLock<RwLock<HashMap<u128, Order>>> = OnceLock::new();

pub fn init_store() {
    ORDERS.set(RwLock::new(HashMap::new())).ok();
}

pub fn add_order(order: Order) {
    let mut orders = ORDERS.get().unwrap().write().unwrap();
    orders.insert(order.id, order);
}

pub fn update_order_server_id(id: u128, server_id: &str) {
    let mut orders = ORDERS.get().unwrap().write().unwrap();
    if let Some(order) = orders.get_mut(&id) {
        order.server_id = Some(server_id.to_string());
    }
}

pub fn update_order_status(id: u128, status: OrderStatus) {
    let mut orders = ORDERS.get().unwrap().write().unwrap();
    if let Some(order) = orders.get_mut(&id) {
        order.status = status;
    }
}

pub fn get_orders() -> Vec<Order> {
    let orders = ORDERS.get().unwrap().read().unwrap();
    orders.values().cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::time::SystemTime;

    static TEST_MUTEX: Mutex<()> = Mutex::new(());

    fn clear_orders() {
        init_store();
        ORDERS.get().unwrap().write().unwrap().clear();
    }

    fn make_order(id: u128, recipe: &str) -> Order {
        Order {
            id,
            server_id: None,
            recipe_name: recipe.to_string(),
            status: OrderStatus::Sending,
            timestamp: SystemTime::now(),
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
}
