/**
 * ZLayer SDK for building WASM plugins in TypeScript.
 *
 * This module provides helper functions that wrap the generated WIT bindings
 * for easier access to ZLayer host capabilities including configuration,
 * key-value storage, logging, secrets, and metrics.
 *
 * @packageDocumentation
 */

// =============================================================================
// Version
// =============================================================================

/**
 * The current version of the ZLayer SDK.
 */
export const VERSION = '0.1.0';

// =============================================================================
// Error Classes
// =============================================================================

/**
 * Error thrown when configuration operations fail.
 */
export class ConfigError extends Error {
    /** The configuration key that caused the error */
    public readonly key: string;

    constructor(key: string, message: string) {
        super(message);
        this.name = 'ConfigError';
        this.key = key;
        Object.setPrototypeOf(this, ConfigError.prototype);
    }
}

/**
 * Error thrown when key-value storage operations fail.
 */
export class KVError extends Error {
    /** The key that caused the error */
    public readonly key: string;
    /** The bucket where the error occurred */
    public readonly bucket: string;
    /** The error code from the host */
    public readonly code: KVErrorCode;

    constructor(bucket: string, key: string, code: KVErrorCode, message: string) {
        super(message);
        this.name = 'KVError';
        this.bucket = bucket;
        this.key = key;
        this.code = code;
        Object.setPrototypeOf(this, KVError.prototype);
    }
}

/**
 * Error codes for key-value operations.
 */
export enum KVErrorCode {
    /** Key not found */
    NotFound = 'not_found',
    /** Value too large */
    ValueTooLarge = 'value_too_large',
    /** Storage quota exceeded */
    QuotaExceeded = 'quota_exceeded',
    /** Key format invalid */
    InvalidKey = 'invalid_key',
    /** Generic storage error */
    Storage = 'storage',
}

/**
 * Error thrown when secret operations fail.
 */
export class SecretError extends Error {
    /** The secret name that caused the error */
    public readonly secretName: string;

    constructor(secretName: string, message: string) {
        super(message);
        this.name = 'SecretError';
        this.secretName = secretName;
        Object.setPrototypeOf(this, SecretError.prototype);
    }
}

/**
 * Error thrown when a required value is missing.
 */
export class NotFoundError extends Error {
    /** The identifier that was not found */
    public readonly identifier: string;

    constructor(identifier: string, message: string) {
        super(message);
        this.name = 'NotFoundError';
        this.identifier = identifier;
        Object.setPrototypeOf(this, NotFoundError.prototype);
    }
}

// =============================================================================
// Log Level Enum
// =============================================================================

/**
 * Log severity levels matching the WIT interface.
 */
export enum LogLevel {
    /** Finest-grained debugging information */
    Trace = 0,
    /** Debugging information */
    Debug = 1,
    /** Informational messages */
    Info = 2,
    /** Warning messages */
    Warn = 3,
    /** Error messages */
    Error = 4,
}

// =============================================================================
// Type Definitions
// =============================================================================

/**
 * A key-value pair used throughout the SDK.
 */
export interface KeyValue {
    key: string;
    value: string;
}

/**
 * HTTP methods supported by the plugin system.
 */
export enum HttpMethod {
    Get = 'GET',
    Post = 'POST',
    Put = 'PUT',
    Delete = 'DELETE',
    Patch = 'PATCH',
    Head = 'HEAD',
    Options = 'OPTIONS',
    Connect = 'CONNECT',
    Trace = 'TRACE',
}

/**
 * Semantic version representation.
 */
export interface Version {
    major: number;
    minor: number;
    patch: number;
    preRelease?: string;
}

/**
 * Plugin information returned by the info() method.
 */
export interface PluginInfo {
    /** Unique plugin identifier (e.g., "zlayer:auth-jwt") */
    id: string;
    /** Human-readable name */
    name: string;
    /** Plugin version */
    version: Version;
    /** Brief description of plugin functionality */
    description: string;
    /** Plugin author or organization */
    author: string;
    /** License identifier (e.g., "MIT", "Apache-2.0") */
    license?: string;
    /** Homepage or repository URL */
    homepage?: string;
    /** Additional metadata as key-value pairs */
    metadata?: KeyValue[];
}

/**
 * Incoming request to be processed by a plugin.
 */
export interface PluginRequest {
    /** Unique request identifier for tracing */
    requestId: string;
    /** Request path (e.g., "/api/users/123") */
    path: string;
    /** HTTP method */
    method: HttpMethod;
    /** Query string (without leading ?) */
    query?: string;
    /** Request headers */
    headers: KeyValue[];
    /** Request body as bytes */
    body: Uint8Array;
    /** Request timestamp in nanoseconds since Unix epoch */
    timestamp: bigint;
    /** Additional context from the host */
    context: KeyValue[];
}

/**
 * Plugin response returned to the host.
 */
export interface PluginResponse {
    /** HTTP status code (200, 404, 500, etc.) */
    status: number;
    /** Response headers */
    headers: KeyValue[];
    /** Response body */
    body: Uint8Array;
}

/**
 * Result of handling a request.
 */
export type HandleResult =
    | { type: 'response'; response: PluginResponse }
    | { type: 'passThrough' }
    | { type: 'error'; message: string };

/**
 * Plugin capabilities that can be requested.
 */
export interface Capabilities {
    config?: boolean;
    keyvalue?: boolean;
    logging?: boolean;
    secrets?: boolean;
    metrics?: boolean;
    httpClient?: boolean;
}

/**
 * Plugin initialization error types.
 */
export type InitError =
    | { type: 'configMissing'; key: string }
    | { type: 'configInvalid'; key: string }
    | { type: 'capabilityUnavailable'; capability: string }
    | { type: 'failed'; message: string };

/**
 * Authentication result from an authenticator plugin.
 */
export type AuthResult =
    | { type: 'authenticated'; claims: KeyValue[] }
    | { type: 'denied'; reason: string }
    | { type: 'challenge'; challenge: string }
    | { type: 'skip' };

/**
 * Rate limit information.
 */
export interface RateLimitInfo {
    /** Requests remaining in current window */
    remaining: bigint;
    /** Total limit for the window */
    limit: bigint;
    /** Time until limit resets (nanoseconds) */
    resetAfter: bigint;
    /** Retry after this duration if denied (nanoseconds) */
    retryAfter?: bigint;
}

/**
 * Rate limit decision.
 */
export type RateLimitResult =
    | { type: 'allowed'; info: RateLimitInfo }
    | { type: 'denied'; info: RateLimitInfo };

// =============================================================================
// Plugin Interfaces
// =============================================================================

/**
 * The main plugin handler interface that plugins must export.
 */
export interface ZLayerPlugin {
    /**
     * Initialize the plugin.
     * Called once when the plugin is loaded.
     * @returns Capabilities the plugin requires, or an error
     */
    init?(): Capabilities | InitError;

    /**
     * Return plugin metadata.
     * Called after successful initialization.
     */
    info(): PluginInfo;

    /**
     * Handle an incoming request.
     * @param request The incoming request to process
     * @returns The result of handling the request
     */
    handle(request: PluginRequest): HandleResult;

    /**
     * Graceful shutdown hook.
     * Called when the plugin is being unloaded.
     */
    shutdown?(): void;
}

/**
 * A simpler plugin interface for stateless transformations.
 */
export interface TransformerPlugin {
    /**
     * Transform request headers before forwarding.
     */
    transformRequestHeaders?(headers: KeyValue[]): KeyValue[];

    /**
     * Transform response headers before returning.
     */
    transformResponseHeaders?(headers: KeyValue[]): KeyValue[];

    /**
     * Transform request body before forwarding.
     */
    transformRequestBody?(body: Uint8Array): Uint8Array;

    /**
     * Transform response body before returning.
     */
    transformResponseBody?(body: Uint8Array): Uint8Array;
}

/**
 * Authentication plugin interface.
 */
export interface AuthenticatorPlugin {
    /**
     * Authenticate an incoming request.
     * @param request The request to authenticate
     * @returns Authentication result with identity claims on success
     */
    authenticate(request: PluginRequest): AuthResult;

    /**
     * Validate a token (e.g., JWT, API key).
     * @param token The token to validate
     * @returns Claims if valid
     * @throws Error if invalid
     */
    validateToken?(token: string): KeyValue[];
}

/**
 * Rate limiting plugin interface.
 */
export interface RateLimiterPlugin {
    /**
     * Check if request should be rate limited.
     * @param request The request to check
     * @returns Rate limit decision
     */
    check(request: PluginRequest): RateLimitResult;

    /**
     * Get current rate limit status for a key.
     * @param key The rate limit key
     * @returns Current status or undefined if not found
     */
    status?(key: string): RateLimitInfo | undefined;
}

// =============================================================================
// Host Bindings Stub
// =============================================================================

/**
 * Stub for host bindings. In actual WASM execution, these are replaced
 * by the generated WIT bindings that call into the host.
 */
const hostBindings = {
    config: {
        get: (_key: string): string | undefined => {
            throw new Error('Host bindings not available: config.get not implemented');
        },
        getRequired: (_key: string): string => {
            throw new Error('Host bindings not available: config.getRequired not implemented');
        },
        getMany: (_keys: string[]): Array<[string, string]> => {
            throw new Error('Host bindings not available: config.getMany not implemented');
        },
        getPrefix: (_prefix: string): Array<[string, string]> => {
            throw new Error('Host bindings not available: config.getPrefix not implemented');
        },
        exists: (_key: string): boolean => {
            throw new Error('Host bindings not available: config.exists not implemented');
        },
        getBool: (_key: string): boolean | undefined => {
            throw new Error('Host bindings not available: config.getBool not implemented');
        },
        getInt: (_key: string): bigint | undefined => {
            throw new Error('Host bindings not available: config.getInt not implemented');
        },
        getFloat: (_key: string): number | undefined => {
            throw new Error('Host bindings not available: config.getFloat not implemented');
        },
    },
    keyvalue: {
        get: (_key: string): Uint8Array | undefined => {
            throw new Error('Host bindings not available: keyvalue.get not implemented');
        },
        getString: (_key: string): string | undefined => {
            throw new Error('Host bindings not available: keyvalue.getString not implemented');
        },
        set: (_key: string, _value: Uint8Array): void => {
            throw new Error('Host bindings not available: keyvalue.set not implemented');
        },
        setString: (_key: string, _value: string): void => {
            throw new Error('Host bindings not available: keyvalue.setString not implemented');
        },
        setWithTtl: (_key: string, _value: Uint8Array, _ttl: bigint): void => {
            throw new Error('Host bindings not available: keyvalue.setWithTtl not implemented');
        },
        delete: (_key: string): boolean => {
            throw new Error('Host bindings not available: keyvalue.delete not implemented');
        },
        exists: (_key: string): boolean => {
            throw new Error('Host bindings not available: keyvalue.exists not implemented');
        },
        listKeys: (_prefix: string): string[] => {
            throw new Error('Host bindings not available: keyvalue.listKeys not implemented');
        },
        increment: (_key: string, _delta: bigint): bigint => {
            throw new Error('Host bindings not available: keyvalue.increment not implemented');
        },
        compareAndSwap: (_key: string, _expected: Uint8Array | undefined, _newValue: Uint8Array): boolean => {
            throw new Error('Host bindings not available: keyvalue.compareAndSwap not implemented');
        },
    },
    logging: {
        log: (_level: LogLevel, _message: string): void => {
            throw new Error('Host bindings not available: logging.log not implemented');
        },
        logStructured: (_level: LogLevel, _message: string, _fields: KeyValue[]): void => {
            throw new Error('Host bindings not available: logging.logStructured not implemented');
        },
        trace: (_message: string): void => {
            throw new Error('Host bindings not available: logging.trace not implemented');
        },
        debug: (_message: string): void => {
            throw new Error('Host bindings not available: logging.debug not implemented');
        },
        info: (_message: string): void => {
            throw new Error('Host bindings not available: logging.info not implemented');
        },
        warn: (_message: string): void => {
            throw new Error('Host bindings not available: logging.warn not implemented');
        },
        error: (_message: string): void => {
            throw new Error('Host bindings not available: logging.error not implemented');
        },
        isEnabled: (_level: LogLevel): boolean => {
            throw new Error('Host bindings not available: logging.isEnabled not implemented');
        },
    },
    secrets: {
        get: (_name: string): string | undefined => {
            throw new Error('Host bindings not available: secrets.get not implemented');
        },
        getRequired: (_name: string): string => {
            throw new Error('Host bindings not available: secrets.getRequired not implemented');
        },
        exists: (_name: string): boolean => {
            throw new Error('Host bindings not available: secrets.exists not implemented');
        },
        listNames: (): string[] => {
            throw new Error('Host bindings not available: secrets.listNames not implemented');
        },
    },
    metrics: {
        counterInc: (_name: string, _value: bigint): void => {
            throw new Error('Host bindings not available: metrics.counterInc not implemented');
        },
        counterIncLabeled: (_name: string, _value: bigint, _labels: KeyValue[]): void => {
            throw new Error('Host bindings not available: metrics.counterIncLabeled not implemented');
        },
        gaugeSet: (_name: string, _value: number): void => {
            throw new Error('Host bindings not available: metrics.gaugeSet not implemented');
        },
        gaugeSetLabeled: (_name: string, _value: number, _labels: KeyValue[]): void => {
            throw new Error('Host bindings not available: metrics.gaugeSetLabeled not implemented');
        },
        gaugeAdd: (_name: string, _delta: number): void => {
            throw new Error('Host bindings not available: metrics.gaugeAdd not implemented');
        },
        histogramObserve: (_name: string, _value: number): void => {
            throw new Error('Host bindings not available: metrics.histogramObserve not implemented');
        },
        histogramObserveLabeled: (_name: string, _value: number, _labels: KeyValue[]): void => {
            throw new Error('Host bindings not available: metrics.histogramObserveLabeled not implemented');
        },
        recordDuration: (_name: string, _durationNs: bigint): void => {
            throw new Error('Host bindings not available: metrics.recordDuration not implemented');
        },
        recordDurationLabeled: (_name: string, _durationNs: bigint, _labels: KeyValue[]): void => {
            throw new Error('Host bindings not available: metrics.recordDurationLabeled not implemented');
        },
    },
};

// =============================================================================
// Configuration Helpers
// =============================================================================

/**
 * Get a configuration value by key.
 *
 * @param key - The configuration key to retrieve
 * @returns The configuration value, or undefined if not found
 *
 * @example
 * ```typescript
 * const dbHost = getConfig('database.host');
 * if (dbHost) {
 *     console.log(`Database host: ${dbHost}`);
 * }
 * ```
 */
export function getConfig(key: string): string | undefined {
    return hostBindings.config.get(key);
}

/**
 * Get a required configuration value by key.
 * Throws ConfigError if the key does not exist.
 *
 * @param key - The configuration key to retrieve
 * @returns The configuration value
 * @throws {ConfigError} If the configuration key does not exist
 *
 * @example
 * ```typescript
 * try {
 *     const apiKey = getConfigRequired('api.key');
 * } catch (e) {
 *     if (e instanceof ConfigError) {
 *         log.error(`Missing config: ${e.key}`);
 *     }
 * }
 * ```
 */
export function getConfigRequired(key: string): string {
    const value = hostBindings.config.get(key);
    if (value === undefined) {
        throw new ConfigError(key, `Required configuration key '${key}' not found`);
    }
    return value;
}

/**
 * Get a configuration value as a boolean.
 * Recognizes: "true", "false", "1", "0", "yes", "no" (case-insensitive).
 *
 * @param key - The configuration key to retrieve
 * @returns The boolean value, or undefined if not found or not parseable
 *
 * @example
 * ```typescript
 * const debugMode = getConfigBool('debug.enabled') ?? false;
 * ```
 */
export function getConfigBool(key: string): boolean | undefined {
    return hostBindings.config.getBool(key);
}

/**
 * Get a configuration value as an integer.
 *
 * @param key - The configuration key to retrieve
 * @returns The integer value, or undefined if not found or not parseable
 *
 * @example
 * ```typescript
 * const maxRetries = getConfigInt('http.max_retries') ?? 3;
 * ```
 */
export function getConfigInt(key: string): number | undefined {
    const value = hostBindings.config.getInt(key);
    if (value === undefined) {
        return undefined;
    }
    return Number(value);
}

/**
 * Get a configuration value as a float.
 *
 * @param key - The configuration key to retrieve
 * @returns The float value, or undefined if not found or not parseable
 *
 * @example
 * ```typescript
 * const timeout = getConfigFloat('http.timeout_seconds') ?? 30.0;
 * ```
 */
export function getConfigFloat(key: string): number | undefined {
    return hostBindings.config.getFloat(key);
}

/**
 * Get multiple configuration values at once.
 *
 * @param keys - The configuration keys to retrieve
 * @returns A Map of key to value for keys that exist
 *
 * @example
 * ```typescript
 * const config = getConfigMany(['db.host', 'db.port', 'db.name']);
 * const host = config.get('db.host') ?? 'localhost';
 * ```
 */
export function getConfigMany(keys: string[]): Map<string, string> {
    const pairs = hostBindings.config.getMany(keys);
    return new Map(pairs);
}

/**
 * Get all configuration keys with a given prefix.
 *
 * @param prefix - The prefix to match (e.g., "database.")
 * @returns A Map of key to value for matching keys
 *
 * @example
 * ```typescript
 * const dbConfig = getConfigPrefix('database.');
 * // Returns: Map { 'database.host' => 'localhost', 'database.port' => '5432' }
 * ```
 */
export function getConfigPrefix(prefix: string): Map<string, string> {
    const pairs = hostBindings.config.getPrefix(prefix);
    return new Map(pairs);
}

/**
 * Check if a configuration key exists.
 *
 * @param key - The configuration key to check
 * @returns true if the key exists, false otherwise
 */
export function configExists(key: string): boolean {
    return hostBindings.config.exists(key);
}

/**
 * Get all configuration as a JSON string.
 * Useful for debugging or serialization.
 *
 * @returns JSON string of all configuration
 *
 * @example
 * ```typescript
 * log.debug(`All config: ${getAllConfig()}`);
 * ```
 */
export function getAllConfig(): string {
    const pairs = hostBindings.config.getPrefix('');
    const obj: Record<string, string> = {};
    for (const [key, value] of pairs) {
        obj[key] = value;
    }
    return JSON.stringify(obj);
}

// =============================================================================
// Key-Value Storage Helpers
// =============================================================================

/**
 * Construct a namespaced key from bucket and key.
 *
 * @param bucket - The bucket/namespace
 * @param key - The key within the bucket
 * @returns The full namespaced key
 */
function makeKey(bucket: string, key: string): string {
    return `${bucket}:${key}`;
}

/**
 * Get a value from key-value storage as bytes.
 *
 * @param bucket - The bucket/namespace
 * @param key - The key to retrieve
 * @returns The value as bytes, or undefined if not found
 *
 * @example
 * ```typescript
 * const data = kvGet('sessions', sessionId);
 * if (data) {
 *     const session = JSON.parse(new TextDecoder().decode(data));
 * }
 * ```
 */
export function kvGet(bucket: string, key: string): Uint8Array | undefined {
    return hostBindings.keyvalue.get(makeKey(bucket, key));
}

/**
 * Get a value from key-value storage as a string.
 *
 * @param bucket - The bucket/namespace
 * @param key - The key to retrieve
 * @returns The value as a string, or undefined if not found
 *
 * @example
 * ```typescript
 * const username = kvGetString('users', `user:${userId}:name`);
 * ```
 */
export function kvGetString(bucket: string, key: string): string | undefined {
    return hostBindings.keyvalue.getString(makeKey(bucket, key));
}

/**
 * Get a value from key-value storage as JSON.
 *
 * @param bucket - The bucket/namespace
 * @param key - The key to retrieve
 * @returns The parsed JSON value, or undefined if not found
 *
 * @example
 * ```typescript
 * interface User { name: string; email: string; }
 * const user = kvGetJson<User>('users', userId);
 * ```
 */
export function kvGetJson<T>(bucket: string, key: string): T | undefined {
    const value = kvGetString(bucket, key);
    if (value === undefined) {
        return undefined;
    }
    return JSON.parse(value) as T;
}

/**
 * Set a value in key-value storage as bytes.
 *
 * @param bucket - The bucket/namespace
 * @param key - The key to set
 * @param value - The value as bytes
 *
 * @example
 * ```typescript
 * const encoder = new TextEncoder();
 * kvSet('sessions', sessionId, encoder.encode(JSON.stringify(session)));
 * ```
 */
export function kvSet(bucket: string, key: string, value: Uint8Array): void {
    hostBindings.keyvalue.set(makeKey(bucket, key), value);
}

/**
 * Set a value in key-value storage as a string.
 *
 * @param bucket - The bucket/namespace
 * @param key - The key to set
 * @param value - The value as a string
 *
 * @example
 * ```typescript
 * kvSetString('users', `user:${userId}:name`, 'John Doe');
 * ```
 */
export function kvSetString(bucket: string, key: string, value: string): void {
    hostBindings.keyvalue.setString(makeKey(bucket, key), value);
}

/**
 * Set a value in key-value storage as JSON.
 *
 * @param bucket - The bucket/namespace
 * @param key - The key to set
 * @param value - The value to serialize as JSON
 *
 * @example
 * ```typescript
 * kvSetJson('users', userId, { name: 'John', email: 'john@example.com' });
 * ```
 */
export function kvSetJson<T>(bucket: string, key: string, value: T): void {
    kvSetString(bucket, key, JSON.stringify(value));
}

/**
 * Set a value in key-value storage with a TTL (time-to-live).
 *
 * @param bucket - The bucket/namespace
 * @param key - The key to set
 * @param value - The value as bytes
 * @param ttlMs - Time-to-live in milliseconds
 *
 * @example
 * ```typescript
 * // Cache for 5 minutes
 * kvSetWithTtl('cache', cacheKey, data, 5 * 60 * 1000);
 * ```
 */
export function kvSetWithTtl(bucket: string, key: string, value: Uint8Array, ttlMs: number): void {
    const ttlNs = BigInt(ttlMs) * BigInt(1_000_000);
    hostBindings.keyvalue.setWithTtl(makeKey(bucket, key), value, ttlNs);
}

/**
 * Set a string value in key-value storage with a TTL.
 *
 * @param bucket - The bucket/namespace
 * @param key - The key to set
 * @param value - The value as a string
 * @param ttlMs - Time-to-live in milliseconds
 */
export function kvSetStringWithTtl(bucket: string, key: string, value: string, ttlMs: number): void {
    const encoder = new TextEncoder();
    kvSetWithTtl(bucket, key, encoder.encode(value), ttlMs);
}

/**
 * Delete a key from key-value storage.
 *
 * @param bucket - The bucket/namespace
 * @param key - The key to delete
 * @returns true if the key was deleted, false if it didn't exist
 *
 * @example
 * ```typescript
 * kvDelete('sessions', sessionId);
 * ```
 */
export function kvDelete(bucket: string, key: string): boolean {
    return hostBindings.keyvalue.delete(makeKey(bucket, key));
}

/**
 * List all keys in a bucket with an optional prefix.
 *
 * @param bucket - The bucket/namespace
 * @param prefix - Optional prefix to filter keys
 * @returns Array of keys matching the prefix
 *
 * @example
 * ```typescript
 * const userKeys = kvKeys('users', 'user:');
 * // Returns: ['user:1', 'user:2', 'user:3']
 * ```
 */
export function kvKeys(bucket: string, prefix: string = ''): string[] {
    const fullPrefix = makeKey(bucket, prefix);
    const keys = hostBindings.keyvalue.listKeys(fullPrefix);
    // Strip the bucket prefix from returned keys
    const bucketPrefix = `${bucket}:`;
    return keys.map((k) => k.startsWith(bucketPrefix) ? k.slice(bucketPrefix.length) : k);
}

/**
 * Check if a key exists in key-value storage.
 *
 * @param bucket - The bucket/namespace
 * @param key - The key to check
 * @returns true if the key exists, false otherwise
 */
export function kvExists(bucket: string, key: string): boolean {
    return hostBindings.keyvalue.exists(makeKey(bucket, key));
}

/**
 * Atomically increment a numeric value in key-value storage.
 *
 * @param bucket - The bucket/namespace
 * @param key - The key to increment
 * @param delta - The amount to increment by (default: 1)
 * @returns The new value after incrementing
 *
 * @example
 * ```typescript
 * const newCount = kvIncrement('counters', 'page_views', 1);
 * ```
 */
export function kvIncrement(bucket: string, key: string, delta: number = 1): bigint {
    return hostBindings.keyvalue.increment(makeKey(bucket, key), BigInt(delta));
}

/**
 * Atomically compare and swap a value in key-value storage.
 *
 * @param bucket - The bucket/namespace
 * @param key - The key to update
 * @param expected - The expected current value (undefined if key shouldn't exist)
 * @param newValue - The new value to set if expected matches
 * @returns true if the swap succeeded, false if current value didn't match
 *
 * @example
 * ```typescript
 * // Optimistic locking
 * const current = kvGet('locks', 'resource');
 * const swapped = kvCompareAndSwap('locks', 'resource', current, newLockValue);
 * if (!swapped) {
 *     throw new Error('Resource was modified by another process');
 * }
 * ```
 */
export function kvCompareAndSwap(
    bucket: string,
    key: string,
    expected: Uint8Array | undefined,
    newValue: Uint8Array
): boolean {
    return hostBindings.keyvalue.compareAndSwap(makeKey(bucket, key), expected, newValue);
}

// =============================================================================
// Logging Helpers
// =============================================================================

/**
 * Logging utilities for emitting structured logs to the host.
 */
export const log = {
    /**
     * Emit a trace-level log message.
     * Use for finest-grained debugging information.
     *
     * @param msg - The log message
     */
    trace: (msg: string): void => {
        hostBindings.logging.trace(msg);
    },

    /**
     * Emit a debug-level log message.
     * Use for debugging information.
     *
     * @param msg - The log message
     */
    debug: (msg: string): void => {
        hostBindings.logging.debug(msg);
    },

    /**
     * Emit an info-level log message.
     * Use for informational messages about normal operation.
     *
     * @param msg - The log message
     */
    info: (msg: string): void => {
        hostBindings.logging.info(msg);
    },

    /**
     * Emit a warn-level log message.
     * Use for warning messages about potential issues.
     *
     * @param msg - The log message
     */
    warn: (msg: string): void => {
        hostBindings.logging.warn(msg);
    },

    /**
     * Emit an error-level log message.
     * Use for error messages.
     *
     * @param msg - The log message
     */
    error: (msg: string): void => {
        hostBindings.logging.error(msg);
    },

    /**
     * Emit a log message at the specified level.
     *
     * @param level - The log level
     * @param msg - The log message
     */
    log: (level: LogLevel, msg: string): void => {
        hostBindings.logging.log(level, msg);
    },

    /**
     * Emit a structured log message with key-value fields.
     *
     * @param level - The log level
     * @param msg - The log message
     * @param fields - Additional structured fields
     *
     * @example
     * ```typescript
     * log.structured(LogLevel.Info, 'Request processed', {
     *     requestId: '123',
     *     duration: '45ms',
     *     status: '200',
     * });
     * ```
     */
    structured: (level: LogLevel, msg: string, fields: Record<string, string>): void => {
        const kvFields: KeyValue[] = Object.entries(fields).map(([key, value]) => ({
            key,
            value,
        }));
        hostBindings.logging.logStructured(level, msg, kvFields);
    },

    /**
     * Check if a log level is enabled.
     * Useful for avoiding expensive log construction when the level is disabled.
     *
     * @param level - The log level to check
     * @returns true if the level is enabled
     *
     * @example
     * ```typescript
     * if (log.isEnabled(LogLevel.Debug)) {
     *     log.debug(`Complex data: ${JSON.stringify(largeObject)}`);
     * }
     * ```
     */
    isEnabled: (level: LogLevel): boolean => {
        return hostBindings.logging.isEnabled(level);
    },
};

// =============================================================================
// Secrets Helpers
// =============================================================================

/**
 * Get a secret by name.
 *
 * @param name - The secret name
 * @returns The secret value, or undefined if not found
 *
 * @example
 * ```typescript
 * const apiKey = getSecret('external_api_key');
 * ```
 */
export function getSecret(name: string): string | undefined {
    return hostBindings.secrets.get(name);
}

/**
 * Get a required secret by name.
 * Throws SecretError if the secret does not exist.
 *
 * @param name - The secret name
 * @returns The secret value
 * @throws {SecretError} If the secret does not exist
 *
 * @example
 * ```typescript
 * const dbPassword = getSecretRequired('database_password');
 * ```
 */
export function getSecretRequired(name: string): string {
    const value = hostBindings.secrets.get(name);
    if (value === undefined) {
        throw new SecretError(name, `Required secret '${name}' not found`);
    }
    return value;
}

/**
 * Check if a secret exists.
 *
 * @param name - The secret name
 * @returns true if the secret exists, false otherwise
 */
export function secretExists(name: string): boolean {
    return hostBindings.secrets.exists(name);
}

/**
 * List all available secret names (not values).
 * Useful for diagnostics without exposing sensitive data.
 *
 * @returns Array of secret names
 */
export function listSecretNames(): string[] {
    return hostBindings.secrets.listNames();
}

// =============================================================================
// Metrics Helpers
// =============================================================================

/**
 * Metrics utilities for emitting observability data to the host.
 */
export const metrics = {
    /**
     * Increment a counter metric.
     *
     * @param name - The metric name
     * @param value - The amount to increment by (default: 1)
     *
     * @example
     * ```typescript
     * metrics.counterInc('requests_total');
     * metrics.counterInc('bytes_processed', byteCount);
     * ```
     */
    counterInc: (name: string, value: number = 1): void => {
        hostBindings.metrics.counterInc(name, BigInt(value));
    },

    /**
     * Increment a counter metric with labels.
     *
     * @param name - The metric name
     * @param value - The amount to increment by
     * @param labels - The metric labels
     *
     * @example
     * ```typescript
     * metrics.counterIncLabeled('http_requests_total', 1, {
     *     method: 'GET',
     *     path: '/api/users',
     *     status: '200',
     * });
     * ```
     */
    counterIncLabeled: (name: string, value: number, labels: Record<string, string>): void => {
        const kvLabels: KeyValue[] = Object.entries(labels).map(([key, val]) => ({
            key,
            value: val,
        }));
        hostBindings.metrics.counterIncLabeled(name, BigInt(value), kvLabels);
    },

    /**
     * Set a gauge metric to a value.
     *
     * @param name - The metric name
     * @param value - The value to set
     *
     * @example
     * ```typescript
     * metrics.gaugeSet('active_connections', connectionCount);
     * ```
     */
    gaugeSet: (name: string, value: number): void => {
        hostBindings.metrics.gaugeSet(name, value);
    },

    /**
     * Set a gauge metric with labels.
     *
     * @param name - The metric name
     * @param value - The value to set
     * @param labels - The metric labels
     */
    gaugeSetLabeled: (name: string, value: number, labels: Record<string, string>): void => {
        const kvLabels: KeyValue[] = Object.entries(labels).map(([key, val]) => ({
            key,
            value: val,
        }));
        hostBindings.metrics.gaugeSetLabeled(name, value, kvLabels);
    },

    /**
     * Add to a gauge value (can be negative).
     *
     * @param name - The metric name
     * @param delta - The amount to add (can be negative)
     */
    gaugeAdd: (name: string, delta: number): void => {
        hostBindings.metrics.gaugeAdd(name, delta);
    },

    /**
     * Record a histogram observation.
     *
     * @param name - The metric name
     * @param value - The observed value
     *
     * @example
     * ```typescript
     * metrics.histogramObserve('request_duration_seconds', 0.045);
     * ```
     */
    histogramObserve: (name: string, value: number): void => {
        hostBindings.metrics.histogramObserve(name, value);
    },

    /**
     * Record a histogram observation with labels.
     *
     * @param name - The metric name
     * @param value - The observed value
     * @param labels - The metric labels
     */
    histogramObserveLabeled: (name: string, value: number, labels: Record<string, string>): void => {
        const kvLabels: KeyValue[] = Object.entries(labels).map(([key, val]) => ({
            key,
            value: val,
        }));
        hostBindings.metrics.histogramObserveLabeled(name, value, kvLabels);
    },

    /**
     * Record request duration in milliseconds.
     * Convenience method that converts to nanoseconds.
     *
     * @param name - The metric name
     * @param durationMs - Duration in milliseconds
     */
    recordDuration: (name: string, durationMs: number): void => {
        const durationNs = BigInt(Math.floor(durationMs * 1_000_000));
        hostBindings.metrics.recordDuration(name, durationNs);
    },

    /**
     * Record request duration with labels.
     *
     * @param name - The metric name
     * @param durationMs - Duration in milliseconds
     * @param labels - The metric labels
     */
    recordDurationLabeled: (name: string, durationMs: number, labels: Record<string, string>): void => {
        const durationNs = BigInt(Math.floor(durationMs * 1_000_000));
        const kvLabels: KeyValue[] = Object.entries(labels).map(([key, val]) => ({
            key,
            value: val,
        }));
        hostBindings.metrics.recordDurationLabeled(name, durationNs, kvLabels);
    },
};

// =============================================================================
// Response Helpers
// =============================================================================

/**
 * Create a successful response with JSON body.
 *
 * @param data - The data to serialize as JSON
 * @param status - HTTP status code (default: 200)
 * @param headers - Additional headers
 * @returns A PluginResponse
 *
 * @example
 * ```typescript
 * return jsonResponse({ users: ['alice', 'bob'] });
 * return jsonResponse({ error: 'Not found' }, 404);
 * ```
 */
export function jsonResponse<T>(
    data: T,
    status: number = 200,
    headers: Record<string, string> = {}
): PluginResponse {
    const body = JSON.stringify(data);
    const encoder = new TextEncoder();
    const responseHeaders: KeyValue[] = [
        { key: 'Content-Type', value: 'application/json' },
        ...Object.entries(headers).map(([key, value]) => ({ key, value })),
    ];

    return {
        status,
        headers: responseHeaders,
        body: encoder.encode(body),
    };
}

/**
 * Create a text response.
 *
 * @param text - The response text
 * @param status - HTTP status code (default: 200)
 * @param contentType - Content type (default: text/plain)
 * @returns A PluginResponse
 */
export function textResponse(
    text: string,
    status: number = 200,
    contentType: string = 'text/plain'
): PluginResponse {
    const encoder = new TextEncoder();
    return {
        status,
        headers: [{ key: 'Content-Type', value: contentType }],
        body: encoder.encode(text),
    };
}

/**
 * Create an HTML response.
 *
 * @param html - The HTML content
 * @param status - HTTP status code (default: 200)
 * @returns A PluginResponse
 */
export function htmlResponse(html: string, status: number = 200): PluginResponse {
    return textResponse(html, status, 'text/html; charset=utf-8');
}

/**
 * Create a binary response.
 *
 * @param data - The binary data
 * @param contentType - Content type
 * @param status - HTTP status code (default: 200)
 * @returns A PluginResponse
 */
export function binaryResponse(
    data: Uint8Array,
    contentType: string,
    status: number = 200
): PluginResponse {
    return {
        status,
        headers: [{ key: 'Content-Type', value: contentType }],
        body: data,
    };
}

/**
 * Create an error response.
 *
 * @param message - The error message
 * @param status - HTTP status code (default: 500)
 * @returns A HandleResult with error response
 */
export function errorResponse(message: string, status: number = 500): HandleResult {
    return {
        type: 'response',
        response: jsonResponse({ error: message }, status),
    };
}

/**
 * Create a redirect response.
 *
 * @param location - The URL to redirect to
 * @param status - HTTP status code (default: 302)
 * @returns A PluginResponse
 */
export function redirectResponse(location: string, status: number = 302): PluginResponse {
    return {
        status,
        headers: [{ key: 'Location', value: location }],
        body: new Uint8Array(0),
    };
}

/**
 * Create a pass-through result.
 * Use this when the plugin doesn't want to handle the request.
 *
 * @returns A HandleResult indicating pass-through
 */
export function passThrough(): HandleResult {
    return { type: 'passThrough' };
}

// =============================================================================
// Request Helpers
// =============================================================================

/**
 * Get a header value from a request (case-insensitive).
 *
 * @param request - The plugin request
 * @param name - The header name
 * @returns The header value, or undefined if not found
 */
export function getHeader(request: PluginRequest, name: string): string | undefined {
    const lowerName = name.toLowerCase();
    const header = request.headers.find((h) => h.key.toLowerCase() === lowerName);
    return header?.value;
}

/**
 * Get all values for a header (case-insensitive).
 *
 * @param request - The plugin request
 * @param name - The header name
 * @returns Array of header values
 */
export function getHeaders(request: PluginRequest, name: string): string[] {
    const lowerName = name.toLowerCase();
    return request.headers
        .filter((h) => h.key.toLowerCase() === lowerName)
        .map((h) => h.value);
}

/**
 * Parse the request body as JSON.
 *
 * @param request - The plugin request
 * @returns The parsed JSON body
 */
export function parseJsonBody<T>(request: PluginRequest): T {
    const decoder = new TextDecoder();
    const text = decoder.decode(request.body);
    return JSON.parse(text) as T;
}

/**
 * Get the request body as a string.
 *
 * @param request - The plugin request
 * @returns The body as a string
 */
export function getBodyString(request: PluginRequest): string {
    const decoder = new TextDecoder();
    return decoder.decode(request.body);
}

/**
 * Parse query string into a Map.
 *
 * @param request - The plugin request
 * @returns Map of query parameters
 */
export function parseQuery(request: PluginRequest): Map<string, string> {
    const params = new Map<string, string>();
    if (!request.query) {
        return params;
    }
    const pairs = request.query.split('&');
    for (const pair of pairs) {
        const [key, value] = pair.split('=');
        if (key) {
            params.set(decodeURIComponent(key), decodeURIComponent(value ?? ''));
        }
    }
    return params;
}

/**
 * Get a context value from a request.
 *
 * @param request - The plugin request
 * @param key - The context key
 * @returns The context value, or undefined if not found
 */
export function getContext(request: PluginRequest, key: string): string | undefined {
    const ctx = request.context.find((c) => c.key === key);
    return ctx?.value;
}

// =============================================================================
// Timing Helpers
// =============================================================================

/**
 * Measure the duration of an async operation and record it as a metric.
 *
 * @param name - The metric name
 * @param fn - The async function to measure
 * @returns The result of the function
 *
 * @example
 * ```typescript
 * const result = await timed('database_query', async () => {
 *     return await db.query('SELECT * FROM users');
 * });
 * ```
 */
export async function timed<T>(name: string, fn: () => Promise<T>): Promise<T> {
    const start = Date.now();
    try {
        return await fn();
    } finally {
        const duration = Date.now() - start;
        metrics.recordDuration(name, duration);
    }
}

/**
 * Measure the duration of a sync operation and record it as a metric.
 *
 * @param name - The metric name
 * @param fn - The function to measure
 * @returns The result of the function
 */
export function timedSync<T>(name: string, fn: () => T): T {
    const start = Date.now();
    try {
        return fn();
    } finally {
        const duration = Date.now() - start;
        metrics.recordDuration(name, duration);
    }
}

// =============================================================================
// Version Helpers
// =============================================================================

/**
 * Create a Version object from a semver string.
 *
 * @param semver - The semver string (e.g., "1.2.3" or "1.2.3-beta.1")
 * @returns A Version object
 */
export function parseVersion(semver: string): Version {
    const [versionPart, preRelease] = semver.split('-', 2);
    const [major, minor, patch] = versionPart.split('.').map(Number);
    return {
        major: major ?? 0,
        minor: minor ?? 0,
        patch: patch ?? 0,
        preRelease,
    };
}

/**
 * Format a Version object as a semver string.
 *
 * @param version - The Version object
 * @returns The semver string
 */
export function formatVersion(version: Version): string {
    const base = `${version.major}.${version.minor}.${version.patch}`;
    return version.preRelease ? `${base}-${version.preRelease}` : base;
}

// =============================================================================
// Plugin Registration Helper
// =============================================================================

/**
 * Register a plugin implementation.
 * This is a convenience function for setting up plugin exports.
 *
 * @param plugin - The plugin implementation
 * @returns The plugin for export
 *
 * @example
 * ```typescript
 * export default registerPlugin({
 *     info() {
 *         return {
 *             id: 'my-org:my-plugin',
 *             name: 'My Plugin',
 *             version: { major: 1, minor: 0, patch: 0 },
 *             description: 'A sample plugin',
 *             author: 'My Org',
 *         };
 *     },
 *     handle(request) {
 *         return { type: 'response', response: jsonResponse({ hello: 'world' }) };
 *     },
 * });
 * ```
 */
export function registerPlugin(plugin: ZLayerPlugin): ZLayerPlugin {
    return plugin;
}

