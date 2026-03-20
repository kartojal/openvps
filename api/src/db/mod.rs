use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::Connection;
use std::sync::Mutex;
use uuid::Uuid;

pub mod models;

use models::{VmRecord, VmStatus};

pub struct Database {
    conn: Mutex<Connection>,
}

impl Database {
    pub fn open(path: &str) -> Result<Self> {
        // Create parent directories if needed
        if let Some(parent) = std::path::Path::new(path).parent() {
            std::fs::create_dir_all(parent)?;
        }

        let conn = Connection::open(path)?;
        let db = Self {
            conn: Mutex::new(conn),
        };
        db.migrate()?;
        Ok(db)
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        let db = Self {
            conn: Mutex::new(conn),
        };
        db.migrate()?;
        Ok(db)
    }

    fn migrate(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS vms (
                id TEXT PRIMARY KEY,
                status TEXT NOT NULL DEFAULT 'pending',
                vcpus INTEGER NOT NULL,
                ram_mb INTEGER NOT NULL,
                disk_gb INTEGER NOT NULL,
                image TEXT NOT NULL,
                ip_addr TEXT,
                ssh_port INTEGER DEFAULT 22,
                tap_device TEXT,
                socket_path TEXT,
                pid INTEGER,
                payment_tx TEXT,
                price_micro INTEGER NOT NULL,
                created_at TEXT NOT NULL,
                expires_at TEXT NOT NULL,
                terminated_at TEXT
            );

            CREATE INDEX IF NOT EXISTS idx_vms_status ON vms(status);
            CREATE INDEX IF NOT EXISTS idx_vms_expires_at ON vms(expires_at);

            CREATE TABLE IF NOT EXISTS ip_allocations (
                ip_addr TEXT PRIMARY KEY,
                vm_id TEXT,
                allocated_at TEXT NOT NULL,
                released_at TEXT,
                FOREIGN KEY (vm_id) REFERENCES vms(id)
            );
            ",
        )?;
        Ok(())
    }

    pub fn insert_vm(&self, vm: &VmRecord) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO vms (id, status, vcpus, ram_mb, disk_gb, image, ip_addr, ssh_port,
             tap_device, socket_path, pid, payment_tx, price_micro, created_at, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            rusqlite::params![
                vm.id.to_string(),
                vm.status.as_str(),
                vm.vcpus,
                vm.ram_mb,
                vm.disk_gb,
                vm.image,
                vm.ip_addr,
                vm.ssh_port,
                vm.tap_device,
                vm.socket_path,
                vm.pid,
                vm.payment_tx,
                vm.price_micro,
                vm.created_at.to_rfc3339(),
                vm.expires_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn update_vm_status(&self, id: &Uuid, status: VmStatus) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let terminated_at = if status == VmStatus::Terminated {
            Some(Utc::now().to_rfc3339())
        } else {
            None
        };
        conn.execute(
            "UPDATE vms SET status = ?1, terminated_at = ?2 WHERE id = ?3",
            rusqlite::params![status.as_str(), terminated_at, id.to_string()],
        )?;
        Ok(())
    }

    pub fn update_vm_runtime(
        &self,
        id: &Uuid,
        ip_addr: &str,
        tap_device: &str,
        socket_path: &str,
        pid: u32,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE vms SET ip_addr = ?1, tap_device = ?2, socket_path = ?3, pid = ?4, status = 'running' WHERE id = ?5",
            rusqlite::params![ip_addr, tap_device, socket_path, pid, id.to_string()],
        )?;
        Ok(())
    }

    pub fn get_vm(&self, id: &Uuid) -> Result<Option<VmRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, status, vcpus, ram_mb, disk_gb, image, ip_addr, ssh_port,
             tap_device, socket_path, pid, payment_tx, price_micro, created_at, expires_at, terminated_at
             FROM vms WHERE id = ?1",
        )?;

        let result = stmt.query_row(rusqlite::params![id.to_string()], |row| {
            Ok(VmRecord {
                id: row.get::<_, String>(0)?.parse().unwrap(),
                status: VmStatus::from_str(&row.get::<_, String>(1)?),
                vcpus: row.get(2)?,
                ram_mb: row.get(3)?,
                disk_gb: row.get(4)?,
                image: row.get(5)?,
                ip_addr: row.get(6)?,
                ssh_port: row.get(7)?,
                tap_device: row.get(8)?,
                socket_path: row.get(9)?,
                pid: row.get(10)?,
                payment_tx: row.get(11)?,
                price_micro: row.get(12)?,
                created_at: row
                    .get::<_, String>(13)?
                    .parse::<DateTime<Utc>>()
                    .unwrap(),
                expires_at: row
                    .get::<_, String>(14)?
                    .parse::<DateTime<Utc>>()
                    .unwrap(),
                terminated_at: row
                    .get::<_, Option<String>>(15)?
                    .and_then(|s| s.parse::<DateTime<Utc>>().ok()),
            })
        });

        match result {
            Ok(vm) => Ok(Some(vm)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn list_expired_running_vms(&self) -> Result<Vec<VmRecord>> {
        let conn = self.conn.lock().unwrap();
        let now = Utc::now().to_rfc3339();
        let mut stmt = conn.prepare(
            "SELECT id, status, vcpus, ram_mb, disk_gb, image, ip_addr, ssh_port,
             tap_device, socket_path, pid, payment_tx, price_micro, created_at, expires_at, terminated_at
             FROM vms WHERE status = 'running' AND expires_at < ?1",
        )?;

        let vms = stmt
            .query_map(rusqlite::params![now], |row| {
                Ok(VmRecord {
                    id: row.get::<_, String>(0)?.parse().unwrap(),
                    status: VmStatus::from_str(&row.get::<_, String>(1)?),
                    vcpus: row.get(2)?,
                    ram_mb: row.get(3)?,
                    disk_gb: row.get(4)?,
                    image: row.get(5)?,
                    ip_addr: row.get(6)?,
                    ssh_port: row.get(7)?,
                    tap_device: row.get(8)?,
                    socket_path: row.get(9)?,
                    pid: row.get(10)?,
                    payment_tx: row.get(11)?,
                    price_micro: row.get(12)?,
                    created_at: row
                        .get::<_, String>(13)?
                        .parse::<DateTime<Utc>>()
                        .unwrap(),
                    expires_at: row
                        .get::<_, String>(14)?
                        .parse::<DateTime<Utc>>()
                        .unwrap(),
                    terminated_at: row
                        .get::<_, Option<String>>(15)?
                        .and_then(|s| s.parse::<DateTime<Utc>>().ok()),
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(vms)
    }

    pub fn allocate_ip(&self, ip_addr: &str, vm_id: &Uuid) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO ip_allocations (ip_addr, vm_id, allocated_at) VALUES (?1, ?2, ?3)",
            rusqlite::params![ip_addr, vm_id.to_string(), Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn release_ip(&self, ip_addr: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE ip_allocations SET released_at = ?1, vm_id = NULL WHERE ip_addr = ?2 AND released_at IS NULL",
            rusqlite::params![Utc::now().to_rfc3339(), ip_addr],
        )?;
        Ok(())
    }

    pub fn get_allocated_ips(&self) -> Result<Vec<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT ip_addr FROM ip_allocations WHERE released_at IS NULL",
        )?;
        let ips = stmt
            .query_map([], |row| row.get(0))?
            .collect::<Result<Vec<String>, _>>()?;
        Ok(ips)
    }
}
