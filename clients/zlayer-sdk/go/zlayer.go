// Package zlayer provides the ZLayer SDK for building WASM plugins in Go.
//
// This package provides helper functions that wrap the generated WIT bindings
// for easier access to ZLayer host capabilities like configuration, key-value
// storage, logging, secrets, and metrics.
//
// The actual bindings are generated using:
//
//	wit-bindgen-go generate --world zlayer-plugin --out gen ../../../wit/zlayer
//
// Usage:
//
//	import zlayer "github.com/zlayer/zlayer-sdk-go"
//
//	func init() {
//	    zlayer.LogInfo("Plugin initializing")
//	    apiKey := zlayer.GetConfigRequired("api_key")
//	}
package zlayer

import (
	"encoding/json"
	"errors"
	"fmt"
	"strconv"
	"time"
)

// =============================================================================
// Error Types
// =============================================================================

// ConfigError represents an error related to configuration access.
type ConfigError struct {
	Key     string
	Message string
}

func (e *ConfigError) Error() string {
	if e.Message != "" {
		return fmt.Sprintf("config error for key %q: %s", e.Key, e.Message)
	}
	return fmt.Sprintf("config key %q not found", e.Key)
}

// KVError represents an error from key-value operations.
type KVError struct {
	Key     string
	Message string
	Code    KVErrorCode
}

// KVErrorCode represents the type of KV error.
type KVErrorCode int

const (
	// KVErrorNotFound indicates the key was not found.
	KVErrorNotFound KVErrorCode = iota
	// KVErrorValueTooLarge indicates the value exceeded size limits.
	KVErrorValueTooLarge
	// KVErrorQuotaExceeded indicates the storage quota was exceeded.
	KVErrorQuotaExceeded
	// KVErrorInvalidKey indicates the key format is invalid.
	KVErrorInvalidKey
	// KVErrorStorage indicates a generic storage error.
	KVErrorStorage
)

func (e *KVError) Error() string {
	return fmt.Sprintf("kv error on key %q: %s (code: %d)", e.Key, e.Message, e.Code)
}

// SecretError represents an error related to secret access.
type SecretError struct {
	Name    string
	Message string
}

func (e *SecretError) Error() string {
	if e.Message != "" {
		return fmt.Sprintf("secret error for %q: %s", e.Name, e.Message)
	}
	return fmt.Sprintf("secret %q not found", e.Name)
}

// InitError represents an error during plugin initialization.
type InitError struct {
	Code    InitErrorCode
	Message string
}

// InitErrorCode represents the type of initialization error.
type InitErrorCode int

const (
	// InitErrorConfigMissing indicates required configuration is missing.
	InitErrorConfigMissing InitErrorCode = iota
	// InitErrorConfigInvalid indicates a configuration value is invalid.
	InitErrorConfigInvalid
	// InitErrorCapabilityUnavailable indicates a required capability is not available.
	InitErrorCapabilityUnavailable
	// InitErrorFailed indicates a generic initialization failure.
	InitErrorFailed
)

func (e *InitError) Error() string {
	return fmt.Sprintf("init error (code %d): %s", e.Code, e.Message)
}

// =============================================================================
// Log Levels
// =============================================================================

// LogLevel represents the severity level for log messages.
type LogLevel int

const (
	// LogLevelTrace is for very detailed debugging information.
	LogLevelTrace LogLevel = iota
	// LogLevelDebug is for debugging information.
	LogLevelDebug
	// LogLevelInfo is for general informational messages.
	LogLevelInfo
	// LogLevelWarn is for warning messages.
	LogLevelWarn
	// LogLevelError is for error messages.
	LogLevelError
)

// String returns the string representation of the log level.
func (l LogLevel) String() string {
	switch l {
	case LogLevelTrace:
		return "TRACE"
	case LogLevelDebug:
		return "DEBUG"
	case LogLevelInfo:
		return "INFO"
	case LogLevelWarn:
		return "WARN"
	case LogLevelError:
		return "ERROR"
	default:
		return "UNKNOWN"
	}
}

// =============================================================================
// Plugin Types
// =============================================================================

// Version represents a semantic version.
type Version struct {
	Major      uint32
	Minor      uint32
	Patch      uint32
	PreRelease string
}

// String returns the string representation of the version.
func (v Version) String() string {
	base := fmt.Sprintf("%d.%d.%d", v.Major, v.Minor, v.Patch)
	if v.PreRelease != "" {
		return base + "-" + v.PreRelease
	}
	return base
}

// PluginInfo contains metadata about a plugin.
type PluginInfo struct {
	// ID is the unique plugin identifier (e.g., "zlayer:auth-jwt").
	ID string
	// Name is the human-readable name.
	Name string
	// Version is the plugin version.
	Version Version
	// Description is a brief description of functionality.
	Description string
	// Author is the plugin author or organization.
	Author string
	// License is the license identifier (e.g., "MIT", "Apache-2.0").
	License string
	// Homepage is the repository or homepage URL.
	Homepage string
	// Metadata contains additional key-value metadata.
	Metadata map[string]string
}

// Capabilities represents the capabilities a plugin requires.
type Capabilities uint32

const (
	// CapabilityConfig allows access to configuration values.
	CapabilityConfig Capabilities = 1 << iota
	// CapabilityKeyValue allows access to key-value storage.
	CapabilityKeyValue
	// CapabilityLogging allows emitting logs.
	CapabilityLogging
	// CapabilitySecrets allows access to secrets management.
	CapabilitySecrets
	// CapabilityMetrics allows emitting metrics.
	CapabilityMetrics
	// CapabilityHTTPClient allows making HTTP requests.
	CapabilityHTTPClient
)

// HTTPMethod represents an HTTP method.
type HTTPMethod int

const (
	HTTPMethodGet HTTPMethod = iota
	HTTPMethodPost
	HTTPMethodPut
	HTTPMethodDelete
	HTTPMethodPatch
	HTTPMethodHead
	HTTPMethodOptions
	HTTPMethodConnect
	HTTPMethodTrace
)

// String returns the string representation of the HTTP method.
func (m HTTPMethod) String() string {
	methods := []string{"GET", "POST", "PUT", "DELETE", "PATCH", "HEAD", "OPTIONS", "CONNECT", "TRACE"}
	if int(m) < len(methods) {
		return methods[m]
	}
	return "UNKNOWN"
}

// PluginRequest represents an incoming request to be processed by a plugin.
type PluginRequest struct {
	// RequestID is the unique request identifier for tracing.
	RequestID string
	// Path is the request path (e.g., "/api/users/123").
	Path string
	// Method is the HTTP method.
	Method HTTPMethod
	// Query is the query string (without leading ?).
	Query string
	// Headers contains request headers.
	Headers map[string]string
	// Body is the request body as bytes.
	Body []byte
	// Timestamp is the request timestamp.
	Timestamp time.Time
	// Context contains additional context from the host.
	Context map[string]string
}

// PluginResponse represents a response returned to the host.
type PluginResponse struct {
	// Status is the HTTP status code (200, 404, 500, etc.).
	Status uint16
	// Headers contains response headers.
	Headers map[string]string
	// Body is the response body.
	Body []byte
}

// HandleResult represents the result of handling a request.
type HandleResult struct {
	kind     handleResultKind
	response *PluginResponse
	error    string
}

type handleResultKind int

const (
	handleResultResponse handleResultKind = iota
	handleResultPassThrough
	handleResultError
)

// Response creates a HandleResult that returns a response.
func Response(resp PluginResponse) HandleResult {
	return HandleResult{kind: handleResultResponse, response: &resp}
}

// PassThrough creates a HandleResult that passes to the next handler.
func PassThrough() HandleResult {
	return HandleResult{kind: handleResultPassThrough}
}

// HandleError creates a HandleResult that indicates an error.
func HandleError(err string) HandleResult {
	return HandleResult{kind: handleResultError, error: err}
}

// IsResponse returns true if this result contains a response.
func (r HandleResult) IsResponse() bool {
	return r.kind == handleResultResponse
}

// IsPassThrough returns true if this result is a pass-through.
func (r HandleResult) IsPassThrough() bool {
	return r.kind == handleResultPassThrough
}

// IsError returns true if this result is an error.
func (r HandleResult) IsError() bool {
	return r.kind == handleResultError
}

// GetResponse returns the response if this is a response result.
func (r HandleResult) GetResponse() *PluginResponse {
	return r.response
}

// GetError returns the error message if this is an error result.
func (r HandleResult) GetError() string {
	return r.error
}

// =============================================================================
// Plugin Interface
// =============================================================================

// Plugin defines the interface that plugins must implement.
type Plugin interface {
	// Init initializes the plugin and returns the capabilities it requires.
	// Called once when the plugin is loaded.
	Init() (Capabilities, error)

	// Info returns plugin metadata.
	// Called after successful initialization.
	Info() PluginInfo

	// Handle processes an incoming request.
	// Called for each request that matches the plugin's routing rules.
	Handle(request PluginRequest) HandleResult

	// Shutdown performs graceful shutdown.
	// Called when the plugin is being unloaded.
	Shutdown()
}

// =============================================================================
// Host Bindings Interface (for dependency injection/testing)
// =============================================================================

// HostBindings defines the interface for host function calls.
// This is implemented by the generated bindings and can be mocked for testing.
type HostBindings interface {
	// Config
	ConfigGet(key string) *string
	ConfigGetRequired(key string) (string, error)
	ConfigGetBool(key string) *bool
	ConfigGetInt(key string) *int64
	ConfigGetFloat(key string) *float64
	ConfigExists(key string) bool
	ConfigGetMany(keys []string) map[string]string
	ConfigGetPrefix(prefix string) map[string]string

	// KV
	KVGet(key string) ([]byte, error)
	KVGetString(key string) (*string, error)
	KVSet(key string, value []byte) error
	KVSetString(key, value string) error
	KVSetWithTTL(key string, value []byte, ttl time.Duration) error
	KVDelete(key string) (bool, error)
	KVExists(key string) bool
	KVListKeys(prefix string) ([]string, error)
	KVIncrement(key string, delta int64) (int64, error)
	KVCompareAndSwap(key string, expected []byte, newValue []byte) (bool, error)

	// Logging
	Log(level LogLevel, message string)
	LogStructured(level LogLevel, message string, fields map[string]string)
	LogIsEnabled(level LogLevel) bool

	// Secrets
	SecretGet(name string) (*string, error)
	SecretGetRequired(name string) (string, error)
	SecretExists(name string) bool
	SecretListNames() []string

	// Metrics
	CounterInc(name string, value uint64)
	CounterIncLabeled(name string, value uint64, labels map[string]string)
	GaugeSet(name string, value float64)
	GaugeSetLabeled(name string, value float64, labels map[string]string)
	GaugeAdd(name string, delta float64)
	HistogramObserve(name string, value float64)
	HistogramObserveLabeled(name string, value float64, labels map[string]string)
	RecordDuration(name string, duration time.Duration)
	RecordDurationLabeled(name string, duration time.Duration, labels map[string]string)
}

// hostBindings is the active host bindings implementation.
// Set via SetHostBindings or uses the default stub implementation.
var hostBindings HostBindings = &stubHostBindings{}

// SetHostBindings sets the host bindings implementation.
// This is called by the generated bindings during initialization.
func SetHostBindings(bindings HostBindings) {
	hostBindings = bindings
}

// =============================================================================
// Configuration Helpers
// =============================================================================

// GetConfig returns a configuration value by key.
// Returns nil if the key doesn't exist.
func GetConfig(key string) *string {
	return hostBindings.ConfigGet(key)
}

// GetConfigRequired returns a configuration value, or an error if not found.
func GetConfigRequired(key string) (string, error) {
	return hostBindings.ConfigGetRequired(key)
}

// GetConfigBool returns a configuration value as a boolean.
// Recognizes: "true", "false", "1", "0", "yes", "no".
// Returns nil if the key doesn't exist or can't be parsed.
func GetConfigBool(key string) *bool {
	return hostBindings.ConfigGetBool(key)
}

// GetConfigInt returns a configuration value as an int64.
// Returns nil if the key doesn't exist or can't be parsed.
func GetConfigInt(key string) *int64 {
	return hostBindings.ConfigGetInt(key)
}

// GetConfigFloat returns a configuration value as a float64.
// Returns nil if the key doesn't exist or can't be parsed.
func GetConfigFloat(key string) *float64 {
	return hostBindings.ConfigGetFloat(key)
}

// GetConfigDefault returns a configuration value, or the default if not found.
func GetConfigDefault(key, defaultValue string) string {
	if v := GetConfig(key); v != nil {
		return *v
	}
	return defaultValue
}

// GetConfigBoolDefault returns a configuration boolean, or the default if not found.
func GetConfigBoolDefault(key string, defaultValue bool) bool {
	if v := GetConfigBool(key); v != nil {
		return *v
	}
	return defaultValue
}

// GetConfigIntDefault returns a configuration int, or the default if not found.
func GetConfigIntDefault(key string, defaultValue int64) int64 {
	if v := GetConfigInt(key); v != nil {
		return *v
	}
	return defaultValue
}

// GetConfigFloatDefault returns a configuration float, or the default if not found.
func GetConfigFloatDefault(key string, defaultValue float64) float64 {
	if v := GetConfigFloat(key); v != nil {
		return *v
	}
	return defaultValue
}

// ConfigExists checks if a configuration key exists.
func ConfigExists(key string) bool {
	return hostBindings.ConfigExists(key)
}

// GetConfigMany gets multiple configuration values at once.
// Returns a map of key to value for keys that exist.
func GetConfigMany(keys []string) map[string]string {
	return hostBindings.ConfigGetMany(keys)
}

// GetConfigPrefix gets all configuration keys with a given prefix.
// Example: GetConfigPrefix("database.") returns all database.* keys.
func GetConfigPrefix(prefix string) map[string]string {
	return hostBindings.ConfigGetPrefix(prefix)
}

// GetAllConfig returns all configuration as a JSON string.
// This is a convenience function for debugging.
func GetAllConfig() string {
	config := GetConfigPrefix("")
	data, err := json.Marshal(config)
	if err != nil {
		return "{}"
	}
	return string(data)
}

// MustGetConfig returns a configuration value or panics if not found.
// Use only during initialization where missing config is fatal.
func MustGetConfig(key string) string {
	v, err := GetConfigRequired(key)
	if err != nil {
		panic(fmt.Sprintf("required config key %q not found", key))
	}
	return v
}

// =============================================================================
// Key-Value Storage Helpers
// =============================================================================

// KVGet retrieves a value by key.
// Returns nil if the key doesn't exist.
func KVGet(key string) []byte {
	data, err := hostBindings.KVGet(key)
	if err != nil {
		return nil
	}
	return data
}

// KVGetString retrieves a value as a string.
// Returns nil if the key doesn't exist.
func KVGetString(key string) *string {
	s, err := hostBindings.KVGetString(key)
	if err != nil {
		return nil
	}
	return s
}

// KVGetWithError retrieves a value by key with error information.
func KVGetWithError(key string) ([]byte, error) {
	return hostBindings.KVGet(key)
}

// KVSet stores a value.
func KVSet(key string, value []byte) error {
	return hostBindings.KVSet(key, value)
}

// KVSetString stores a string value.
func KVSetString(key, value string) error {
	return hostBindings.KVSetString(key, value)
}

// KVSetWithTTL stores a value with a time-to-live.
func KVSetWithTTL(key string, value []byte, ttl time.Duration) error {
	return hostBindings.KVSetWithTTL(key, value, ttl)
}

// KVDelete removes a key.
// Returns true if the key existed and was deleted.
func KVDelete(key string) (bool, error) {
	return hostBindings.KVDelete(key)
}

// KVExists checks if a key exists.
func KVExists(key string) bool {
	return hostBindings.KVExists(key)
}

// KVKeys lists all keys with a given prefix.
func KVKeys(prefix string) []string {
	keys, err := hostBindings.KVListKeys(prefix)
	if err != nil {
		return nil
	}
	return keys
}

// KVKeysWithError lists all keys with a given prefix, with error information.
func KVKeysWithError(prefix string) ([]string, error) {
	return hostBindings.KVListKeys(prefix)
}

// KVIncrement atomically increments a numeric value.
// Returns the new value after increment.
func KVIncrement(key string, delta int64) (int64, error) {
	return hostBindings.KVIncrement(key, delta)
}

// KVCompareAndSwap sets a value only if the current value matches expected.
// Returns true if the swap succeeded.
func KVCompareAndSwap(key string, expected, newValue []byte) (bool, error) {
	return hostBindings.KVCompareAndSwap(key, expected, newValue)
}

// KVGetJSON retrieves a value and unmarshals it as JSON.
func KVGetJSON(key string, v any) error {
	data := KVGet(key)
	if data == nil {
		return &KVError{Key: key, Code: KVErrorNotFound, Message: "key not found"}
	}
	return json.Unmarshal(data, v)
}

// KVSetJSON marshals a value as JSON and stores it.
func KVSetJSON(key string, v any) error {
	data, err := json.Marshal(v)
	if err != nil {
		return fmt.Errorf("marshal value: %w", err)
	}
	return KVSet(key, data)
}

// =============================================================================
// Logging Helpers
// =============================================================================

// Log emits a log message at the specified level.
func Log(level LogLevel, msg string) {
	hostBindings.Log(level, msg)
}

// LogTrace emits a trace-level log message.
func LogTrace(msg string) {
	hostBindings.Log(LogLevelTrace, msg)
}

// LogDebug emits a debug-level log message.
func LogDebug(msg string) {
	hostBindings.Log(LogLevelDebug, msg)
}

// LogInfo emits an info-level log message.
func LogInfo(msg string) {
	hostBindings.Log(LogLevelInfo, msg)
}

// LogWarn emits a warning-level log message.
func LogWarn(msg string) {
	hostBindings.Log(LogLevelWarn, msg)
}

// LogError emits an error-level log message.
func LogError(msg string) {
	hostBindings.Log(LogLevelError, msg)
}

// Logf emits a formatted log message at the specified level.
func Logf(level LogLevel, format string, args ...any) {
	hostBindings.Log(level, fmt.Sprintf(format, args...))
}

// LogTracef emits a formatted trace-level log message.
func LogTracef(format string, args ...any) {
	hostBindings.Log(LogLevelTrace, fmt.Sprintf(format, args...))
}

// LogDebugf emits a formatted debug-level log message.
func LogDebugf(format string, args ...any) {
	hostBindings.Log(LogLevelDebug, fmt.Sprintf(format, args...))
}

// LogInfof emits a formatted info-level log message.
func LogInfof(format string, args ...any) {
	hostBindings.Log(LogLevelInfo, fmt.Sprintf(format, args...))
}

// LogWarnf emits a formatted warning-level log message.
func LogWarnf(format string, args ...any) {
	hostBindings.Log(LogLevelWarn, fmt.Sprintf(format, args...))
}

// LogErrorf emits a formatted error-level log message.
func LogErrorf(format string, args ...any) {
	hostBindings.Log(LogLevelError, fmt.Sprintf(format, args...))
}

// LogStructured emits a structured log with key-value fields.
func LogStructured(level LogLevel, msg string, fields map[string]string) {
	hostBindings.LogStructured(level, msg, fields)
}

// LogIsEnabled checks if a log level is enabled.
// Use this before constructing expensive log messages.
func LogIsEnabled(level LogLevel) bool {
	return hostBindings.LogIsEnabled(level)
}

// LogFields is a helper for building log field maps.
type LogFields map[string]string

// NewLogFields creates a new LogFields map.
func NewLogFields() LogFields {
	return make(LogFields)
}

// Set adds or updates a field.
func (f LogFields) Set(key, value string) LogFields {
	f[key] = value
	return f
}

// SetInt adds an integer field.
func (f LogFields) SetInt(key string, value int64) LogFields {
	f[key] = strconv.FormatInt(value, 10)
	return f
}

// SetFloat adds a float field.
func (f LogFields) SetFloat(key string, value float64) LogFields {
	f[key] = strconv.FormatFloat(value, 'f', -1, 64)
	return f
}

// SetBool adds a boolean field.
func (f LogFields) SetBool(key string, value bool) LogFields {
	f[key] = strconv.FormatBool(value)
	return f
}

// SetDuration adds a duration field.
func (f LogFields) SetDuration(key string, value time.Duration) LogFields {
	f[key] = value.String()
	return f
}

// SetError adds an error field.
func (f LogFields) SetError(err error) LogFields {
	if err != nil {
		f["error"] = err.Error()
	}
	return f
}

// =============================================================================
// Secrets Helpers
// =============================================================================

// GetSecret retrieves a secret by name.
// Returns nil if the secret doesn't exist.
func GetSecret(name string) *string {
	s, err := hostBindings.SecretGet(name)
	if err != nil {
		return nil
	}
	return s
}

// GetSecretRequired retrieves a secret, or returns an error if not found.
func GetSecretRequired(name string) (string, error) {
	return hostBindings.SecretGetRequired(name)
}

// SecretExists checks if a secret exists.
func SecretExists(name string) bool {
	return hostBindings.SecretExists(name)
}

// ListSecretNames lists available secret names (not values).
func ListSecretNames() []string {
	return hostBindings.SecretListNames()
}

// MustGetSecret retrieves a secret or panics if not found.
// Use only during initialization where missing secrets are fatal.
func MustGetSecret(name string) string {
	s, err := GetSecretRequired(name)
	if err != nil {
		panic(fmt.Sprintf("required secret %q not found", name))
	}
	return s
}

// =============================================================================
// Metrics Helpers
// =============================================================================

// CounterInc increments a counter metric.
func CounterInc(name string, value uint64) {
	hostBindings.CounterInc(name, value)
}

// CounterIncOne increments a counter by 1.
func CounterIncOne(name string) {
	hostBindings.CounterInc(name, 1)
}

// CounterIncLabeled increments a counter with labels.
func CounterIncLabeled(name string, value uint64, labels map[string]string) {
	hostBindings.CounterIncLabeled(name, value, labels)
}

// GaugeSet sets a gauge metric to a value.
func GaugeSet(name string, value float64) {
	hostBindings.GaugeSet(name, value)
}

// GaugeSetLabeled sets a gauge with labels.
func GaugeSetLabeled(name string, value float64, labels map[string]string) {
	hostBindings.GaugeSetLabeled(name, value, labels)
}

// GaugeAdd adds to a gauge value (can be negative).
func GaugeAdd(name string, delta float64) {
	hostBindings.GaugeAdd(name, delta)
}

// HistogramObserve records a histogram observation.
func HistogramObserve(name string, value float64) {
	hostBindings.HistogramObserve(name, value)
}

// HistogramObserveLabeled records a histogram observation with labels.
func HistogramObserveLabeled(name string, value float64, labels map[string]string) {
	hostBindings.HistogramObserveLabeled(name, value, labels)
}

// RecordDuration records a request duration.
func RecordDuration(name string, duration time.Duration) {
	hostBindings.RecordDuration(name, duration)
}

// RecordDurationLabeled records a request duration with labels.
func RecordDurationLabeled(name string, duration time.Duration, labels map[string]string) {
	hostBindings.RecordDurationLabeled(name, duration, labels)
}

// Labels is a helper for building metric label maps.
type Labels map[string]string

// NewLabels creates a new Labels map.
func NewLabels() Labels {
	return make(Labels)
}

// Set adds or updates a label.
func (l Labels) Set(key, value string) Labels {
	l[key] = value
	return l
}

// Timer provides a convenient way to measure and record durations.
type Timer struct {
	name   string
	start  time.Time
	labels map[string]string
}

// StartTimer starts a new timer for the given metric name.
func StartTimer(name string) *Timer {
	return &Timer{
		name:  name,
		start: time.Now(),
	}
}

// StartTimerLabeled starts a new timer with labels.
func StartTimerLabeled(name string, labels map[string]string) *Timer {
	return &Timer{
		name:   name,
		start:  time.Now(),
		labels: labels,
	}
}

// Stop stops the timer and records the duration.
func (t *Timer) Stop() time.Duration {
	duration := time.Since(t.start)
	if t.labels != nil {
		RecordDurationLabeled(t.name, duration, t.labels)
	} else {
		RecordDuration(t.name, duration)
	}
	return duration
}

// Elapsed returns the elapsed time without recording.
func (t *Timer) Elapsed() time.Duration {
	return time.Since(t.start)
}

// =============================================================================
// HTTP Response Helpers
// =============================================================================

// JSONResponse creates a PluginResponse with JSON content.
func JSONResponse(status uint16, data any) (PluginResponse, error) {
	body, err := json.Marshal(data)
	if err != nil {
		return PluginResponse{}, fmt.Errorf("marshal JSON response: %w", err)
	}
	return PluginResponse{
		Status: status,
		Headers: map[string]string{
			"Content-Type": "application/json",
		},
		Body: body,
	}, nil
}

// TextResponse creates a PluginResponse with text content.
func TextResponse(status uint16, text string) PluginResponse {
	return PluginResponse{
		Status: status,
		Headers: map[string]string{
			"Content-Type": "text/plain; charset=utf-8",
		},
		Body: []byte(text),
	}
}

// HTMLResponse creates a PluginResponse with HTML content.
func HTMLResponse(status uint16, html string) PluginResponse {
	return PluginResponse{
		Status: status,
		Headers: map[string]string{
			"Content-Type": "text/html; charset=utf-8",
		},
		Body: []byte(html),
	}
}

// EmptyResponse creates a PluginResponse with no body.
func EmptyResponse(status uint16) PluginResponse {
	return PluginResponse{
		Status:  status,
		Headers: make(map[string]string),
		Body:    nil,
	}
}

// RedirectResponse creates a redirect response.
func RedirectResponse(location string, permanent bool) PluginResponse {
	status := uint16(302)
	if permanent {
		status = 301
	}
	return PluginResponse{
		Status: status,
		Headers: map[string]string{
			"Location": location,
		},
		Body: nil,
	}
}

// ErrorResponse creates an error response with a JSON body.
func ErrorResponse(status uint16, code, message string) PluginResponse {
	body, _ := json.Marshal(map[string]string{
		"error":   code,
		"message": message,
	})
	return PluginResponse{
		Status: status,
		Headers: map[string]string{
			"Content-Type": "application/json",
		},
		Body: body,
	}
}

// =============================================================================
// Request Helpers
// =============================================================================

// GetHeader returns a header value from the request (case-insensitive).
func (r *PluginRequest) GetHeader(name string) string {
	// Headers are stored as-is, but we do case-insensitive lookup
	for k, v := range r.Headers {
		if equalFoldASCII(k, name) {
			return v
		}
	}
	return ""
}

// GetQueryParam returns a query parameter value.
// For complex query parsing, use net/url.ParseQuery.
func (r *PluginRequest) GetQueryParam(name string) string {
	// Simple implementation - for complex queries use url.ParseQuery
	query := r.Query
	for len(query) > 0 {
		key := query
		if i := indexOf(key, '&'); i >= 0 {
			key, query = key[:i], key[i+1:]
		} else {
			query = ""
		}
		if key == "" {
			continue
		}
		value := ""
		if i := indexOf(key, '='); i >= 0 {
			key, value = key[:i], key[i+1:]
		}
		if key == name {
			return value
		}
	}
	return ""
}

// BodyJSON parses the request body as JSON.
func (r *PluginRequest) BodyJSON(v any) error {
	if len(r.Body) == 0 {
		return errors.New("empty request body")
	}
	return json.Unmarshal(r.Body, v)
}

// BodyString returns the request body as a string.
func (r *PluginRequest) BodyString() string {
	return string(r.Body)
}

// =============================================================================
// Stub Host Bindings (for compilation without generated bindings)
// =============================================================================

// stubHostBindings provides no-op implementations for when bindings aren't available.
type stubHostBindings struct{}

func (s *stubHostBindings) ConfigGet(key string) *string { return nil }
func (s *stubHostBindings) ConfigGetRequired(key string) (string, error) {
	return "", &ConfigError{Key: key}
}
func (s *stubHostBindings) ConfigGetBool(key string) *bool     { return nil }
func (s *stubHostBindings) ConfigGetInt(key string) *int64     { return nil }
func (s *stubHostBindings) ConfigGetFloat(key string) *float64 { return nil }
func (s *stubHostBindings) ConfigExists(key string) bool       { return false }
func (s *stubHostBindings) ConfigGetMany(keys []string) map[string]string {
	return make(map[string]string)
}
func (s *stubHostBindings) ConfigGetPrefix(prefix string) map[string]string {
	return make(map[string]string)
}
func (s *stubHostBindings) KVGet(key string) ([]byte, error) {
	return nil, &KVError{Key: key, Code: KVErrorNotFound, Message: "stub"}
}
func (s *stubHostBindings) KVGetString(key string) (*string, error) {
	return nil, &KVError{Key: key, Code: KVErrorNotFound, Message: "stub"}
}
func (s *stubHostBindings) KVSet(key string, value []byte) error { return nil }
func (s *stubHostBindings) KVSetString(key, value string) error  { return nil }
func (s *stubHostBindings) KVSetWithTTL(key string, value []byte, ttl time.Duration) error {
	return nil
}
func (s *stubHostBindings) KVDelete(key string) (bool, error)                  { return false, nil }
func (s *stubHostBindings) KVExists(key string) bool                           { return false }
func (s *stubHostBindings) KVListKeys(prefix string) ([]string, error)         { return nil, nil }
func (s *stubHostBindings) KVIncrement(key string, delta int64) (int64, error) { return delta, nil }
func (s *stubHostBindings) KVCompareAndSwap(key string, expected []byte, newValue []byte) (bool, error) {
	return false, nil
}
func (s *stubHostBindings) Log(level LogLevel, message string)                                     {}
func (s *stubHostBindings) LogStructured(level LogLevel, message string, fields map[string]string) {}
func (s *stubHostBindings) LogIsEnabled(level LogLevel) bool                                       { return true }
func (s *stubHostBindings) SecretGet(name string) (*string, error) {
	return nil, &SecretError{Name: name}
}
func (s *stubHostBindings) SecretGetRequired(name string) (string, error) {
	return "", &SecretError{Name: name}
}
func (s *stubHostBindings) SecretExists(name string) bool                                         { return false }
func (s *stubHostBindings) SecretListNames() []string                                             { return nil }
func (s *stubHostBindings) CounterInc(name string, value uint64)                                  {}
func (s *stubHostBindings) CounterIncLabeled(name string, value uint64, labels map[string]string) {}
func (s *stubHostBindings) GaugeSet(name string, value float64)                                   {}
func (s *stubHostBindings) GaugeSetLabeled(name string, value float64, labels map[string]string)  {}
func (s *stubHostBindings) GaugeAdd(name string, delta float64)                                   {}
func (s *stubHostBindings) HistogramObserve(name string, value float64)                           {}
func (s *stubHostBindings) HistogramObserveLabeled(name string, value float64, labels map[string]string) {
}
func (s *stubHostBindings) RecordDuration(name string, duration time.Duration) {}
func (s *stubHostBindings) RecordDurationLabeled(name string, duration time.Duration, labels map[string]string) {
}

// =============================================================================
// Utility Functions
// =============================================================================

// equalFoldASCII is a simple ASCII case-insensitive comparison.
func equalFoldASCII(a, b string) bool {
	if len(a) != len(b) {
		return false
	}
	for i := 0; i < len(a); i++ {
		ca, cb := a[i], b[i]
		if ca >= 'A' && ca <= 'Z' {
			ca += 'a' - 'A'
		}
		if cb >= 'A' && cb <= 'Z' {
			cb += 'a' - 'A'
		}
		if ca != cb {
			return false
		}
	}
	return true
}

// indexOf returns the index of c in s, or -1 if not found.
func indexOf(s string, c byte) int {
	for i := 0; i < len(s); i++ {
		if s[i] == c {
			return i
		}
	}
	return -1
}
