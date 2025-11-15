/**
 * Node.js/V8 Warmup Script
 *
 * This script exercises the V8 optimization and module loading subsystem
 * to bring the runtime to a "hot" state before serving real build requests.
 */

// Pre-load common modules to populate V8 cache
const commonModules = [
    'fs',
    'path',
    'child_process',
    'crypto',
    'util',
    'stream',
    'os',
    'events',
];

console.log('=== Starting Node.js/V8 Warmup ===');

// Load common built-in modules
console.log('Loading common built-in modules...');
commonModules.forEach(modName => {
    try {
        require(modName);
    } catch (e) {
        // Module might not exist in all Node versions
        console.warn(`Could not load ${modName}: ${e.message}`);
    }
});

// Try to load common build tool modules (if installed)
const buildModules = ['typescript', 'esbuild'];
buildModules.forEach(modName => {
    try {
        require(modName);
        console.log(`Loaded ${modName}`);
    } catch (e) {
        // These are optional
    }
});

// Exercise hot code paths for V8 TurboFan optimization
function hotFunction() {
    let result = 0;
    for (let i = 0; i < 10000; i++) {
        result += Math.sqrt(i) * Math.random();
        result = Math.sin(result) + Math.cos(result);
    }
    return result;
}

console.log('Running hot function iterations to trigger TurboFan optimization...');
// Run hot function many times to trigger optimization
for (let i = 0; i < 1000; i++) {
    hotFunction();
    if (i % 100 === 0) {
        console.log(`Optimization iteration ${i}/1000`);
    }
}

// Simulate typical build operations
console.log('Simulating typical build operations...');
const operations = [
    () => JSON.parse('{"test": "value", "nested": {"key": 123}}'),
    () => JSON.stringify({ test: 'value', array: [1, 2, 3] }),
    () => Buffer.from('test data').toString('base64'),
    () => Buffer.from('dGVzdCBkYXRh', 'base64').toString('utf8'),
    () => new Date().toISOString(),
    () => require('path').join('/tmp', 'test', 'file.txt'),
    () => require('crypto').createHash('sha256').update('test').digest('hex'),
];

operations.forEach((op, idx) => {
    for (let i = 0; i < 100; i++) {
        try {
            op();
        } catch (e) {
            console.warn(`Operation ${idx} failed: ${e.message}`);
        }
    }
});

// String operations
console.log('Exercising string operations...');
for (let i = 0; i < 1000; i++) {
    let str = `iteration_${i}_test_string`;
    str = str.toUpperCase();
    str = str.toLowerCase();
    str = str.replace(/_/g, '-');
    str.split('-').join('_');
}

// Array and object operations
console.log('Exercising collection operations...');
const arr = Array.from({ length: 1000 }, (_, i) => `item_${i}`);
arr.filter(s => s.includes('5'))
   .map(s => s.toUpperCase())
   .forEach(() => {});

const obj = Object.fromEntries(
    Array.from({ length: 1000 }, (_, i) => [`key_${i}`, i])
);
Object.keys(obj).forEach(() => {});
Object.values(obj).forEach(() => {});

// File system operations
console.log('Exercising filesystem operations...');
const fs = require('fs');
const path = require('path');
const os = require('os');

try {
    const tmpFile = path.join(os.tmpdir(), `warmup-${Date.now()}.tmp`);
    fs.writeFileSync(tmpFile, 'warmup data\n'.repeat(100));
    const content = fs.readFileSync(tmpFile, 'utf8');
    fs.unlinkSync(tmpFile);
} catch (e) {
    console.warn(`Filesystem warmup failed: ${e.message}`);
}

// Force GC if available
if (global.gc) {
    console.log('Triggering garbage collection...');
    global.gc();
}

console.log('=== Node.js/V8 Warmup Complete ===');
