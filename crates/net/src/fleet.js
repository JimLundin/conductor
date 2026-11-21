const databaseName = "conductor-device";
const storeName = "identity";
const deviceKey = "owner";

function open() {
  return new Promise((resolve, reject) => {
    const request = indexedDB.open(databaseName, 1);
    request.onupgradeneeded = () => request.result.createObjectStore(storeName);
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
}

function lookup(db) {
  return new Promise((resolve, reject) => {
    const request = db.transaction(storeName).objectStore(storeName).get(deviceKey);
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
}

function insert(db, key) {
  return new Promise((resolve, reject) => {
    const transaction = db.transaction(storeName, "readwrite");
    transaction.objectStore(storeName).add(key, deviceKey);
    transaction.oncomplete = () => resolve(key);
    transaction.onabort = () => reject(transaction.error);
  });
}

async function identity() {
  const db = await open();
  try {
    const existing = await lookup(db);
    if (existing) return existing;
    const created = await crypto.subtle.generateKey("Ed25519", false, ["sign", "verify"]);
    try {
      return await insert(db, created);
    } catch (error) {
      const concurrent = await lookup(db);
      if (concurrent) return concurrent;
      throw error;
    }
  } finally {
    db.close();
  }
}

export async function fleetPublicKey() {
  const key = await identity();
  return await crypto.subtle.exportKey("raw", key.publicKey);
}

export async function signFleet(data) {
  const key = await identity();
  return await crypto.subtle.sign("Ed25519", key.privateKey, data);
}
