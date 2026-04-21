# HealthCheckRequest

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Command** | Pointer to **[]string** | Command argv (required when &#x60;type &#x3D;&#x3D; \&quot;command\&quot;&#x60;). Joined with spaces and passed to &#x60;sh -c&#x60;. | [optional] 
**ExpectStatus** | Pointer to **NullableInt32** | HTTP status code expected from &#x60;url&#x60; (defaults to 200). | [optional] 
**Interval** | Pointer to **NullableString** | Interval between checks, humantime format (e.g. &#x60;\&quot;30s\&quot;&#x60;). Defaults to 30s. | [optional] 
**Port** | Pointer to **NullableInt32** | TCP port (required when &#x60;type &#x3D;&#x3D; \&quot;tcp\&quot;&#x60;). | [optional] 
**Retries** | Pointer to **NullableInt32** | Number of consecutive failures before marking unhealthy. Defaults to 3. | [optional] 
**StartPeriod** | Pointer to **NullableString** | Grace period before the first check runs, humantime format. Maps to &#x60;HealthSpec::start_grace&#x60;. | [optional] 
**Timeout** | Pointer to **NullableString** | Timeout per individual check, humantime format. | [optional] 
**Type** | **string** | Check variant: &#x60;\&quot;tcp\&quot;&#x60;, &#x60;\&quot;http\&quot;&#x60;, or &#x60;\&quot;command\&quot;&#x60;. | 
**Url** | Pointer to **NullableString** | HTTP URL (required when &#x60;type &#x3D;&#x3D; \&quot;http\&quot;&#x60;). | [optional] 

## Methods

### NewHealthCheckRequest

`func NewHealthCheckRequest(type_ string, ) *HealthCheckRequest`

NewHealthCheckRequest instantiates a new HealthCheckRequest object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewHealthCheckRequestWithDefaults

`func NewHealthCheckRequestWithDefaults() *HealthCheckRequest`

NewHealthCheckRequestWithDefaults instantiates a new HealthCheckRequest object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetCommand

`func (o *HealthCheckRequest) GetCommand() []string`

GetCommand returns the Command field if non-nil, zero value otherwise.

### GetCommandOk

`func (o *HealthCheckRequest) GetCommandOk() (*[]string, bool)`

GetCommandOk returns a tuple with the Command field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetCommand

`func (o *HealthCheckRequest) SetCommand(v []string)`

SetCommand sets Command field to given value.

### HasCommand

`func (o *HealthCheckRequest) HasCommand() bool`

HasCommand returns a boolean if a field has been set.

### SetCommandNil

`func (o *HealthCheckRequest) SetCommandNil(b bool)`

 SetCommandNil sets the value for Command to be an explicit nil

### UnsetCommand
`func (o *HealthCheckRequest) UnsetCommand()`

UnsetCommand ensures that no value is present for Command, not even an explicit nil
### GetExpectStatus

`func (o *HealthCheckRequest) GetExpectStatus() int32`

GetExpectStatus returns the ExpectStatus field if non-nil, zero value otherwise.

### GetExpectStatusOk

`func (o *HealthCheckRequest) GetExpectStatusOk() (*int32, bool)`

GetExpectStatusOk returns a tuple with the ExpectStatus field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetExpectStatus

`func (o *HealthCheckRequest) SetExpectStatus(v int32)`

SetExpectStatus sets ExpectStatus field to given value.

### HasExpectStatus

`func (o *HealthCheckRequest) HasExpectStatus() bool`

HasExpectStatus returns a boolean if a field has been set.

### SetExpectStatusNil

`func (o *HealthCheckRequest) SetExpectStatusNil(b bool)`

 SetExpectStatusNil sets the value for ExpectStatus to be an explicit nil

### UnsetExpectStatus
`func (o *HealthCheckRequest) UnsetExpectStatus()`

UnsetExpectStatus ensures that no value is present for ExpectStatus, not even an explicit nil
### GetInterval

`func (o *HealthCheckRequest) GetInterval() string`

GetInterval returns the Interval field if non-nil, zero value otherwise.

### GetIntervalOk

`func (o *HealthCheckRequest) GetIntervalOk() (*string, bool)`

GetIntervalOk returns a tuple with the Interval field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetInterval

`func (o *HealthCheckRequest) SetInterval(v string)`

SetInterval sets Interval field to given value.

### HasInterval

`func (o *HealthCheckRequest) HasInterval() bool`

HasInterval returns a boolean if a field has been set.

### SetIntervalNil

`func (o *HealthCheckRequest) SetIntervalNil(b bool)`

 SetIntervalNil sets the value for Interval to be an explicit nil

### UnsetInterval
`func (o *HealthCheckRequest) UnsetInterval()`

UnsetInterval ensures that no value is present for Interval, not even an explicit nil
### GetPort

`func (o *HealthCheckRequest) GetPort() int32`

GetPort returns the Port field if non-nil, zero value otherwise.

### GetPortOk

`func (o *HealthCheckRequest) GetPortOk() (*int32, bool)`

GetPortOk returns a tuple with the Port field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetPort

`func (o *HealthCheckRequest) SetPort(v int32)`

SetPort sets Port field to given value.

### HasPort

`func (o *HealthCheckRequest) HasPort() bool`

HasPort returns a boolean if a field has been set.

### SetPortNil

`func (o *HealthCheckRequest) SetPortNil(b bool)`

 SetPortNil sets the value for Port to be an explicit nil

### UnsetPort
`func (o *HealthCheckRequest) UnsetPort()`

UnsetPort ensures that no value is present for Port, not even an explicit nil
### GetRetries

`func (o *HealthCheckRequest) GetRetries() int32`

GetRetries returns the Retries field if non-nil, zero value otherwise.

### GetRetriesOk

`func (o *HealthCheckRequest) GetRetriesOk() (*int32, bool)`

GetRetriesOk returns a tuple with the Retries field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetRetries

`func (o *HealthCheckRequest) SetRetries(v int32)`

SetRetries sets Retries field to given value.

### HasRetries

`func (o *HealthCheckRequest) HasRetries() bool`

HasRetries returns a boolean if a field has been set.

### SetRetriesNil

`func (o *HealthCheckRequest) SetRetriesNil(b bool)`

 SetRetriesNil sets the value for Retries to be an explicit nil

### UnsetRetries
`func (o *HealthCheckRequest) UnsetRetries()`

UnsetRetries ensures that no value is present for Retries, not even an explicit nil
### GetStartPeriod

`func (o *HealthCheckRequest) GetStartPeriod() string`

GetStartPeriod returns the StartPeriod field if non-nil, zero value otherwise.

### GetStartPeriodOk

`func (o *HealthCheckRequest) GetStartPeriodOk() (*string, bool)`

GetStartPeriodOk returns a tuple with the StartPeriod field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetStartPeriod

`func (o *HealthCheckRequest) SetStartPeriod(v string)`

SetStartPeriod sets StartPeriod field to given value.

### HasStartPeriod

`func (o *HealthCheckRequest) HasStartPeriod() bool`

HasStartPeriod returns a boolean if a field has been set.

### SetStartPeriodNil

`func (o *HealthCheckRequest) SetStartPeriodNil(b bool)`

 SetStartPeriodNil sets the value for StartPeriod to be an explicit nil

### UnsetStartPeriod
`func (o *HealthCheckRequest) UnsetStartPeriod()`

UnsetStartPeriod ensures that no value is present for StartPeriod, not even an explicit nil
### GetTimeout

`func (o *HealthCheckRequest) GetTimeout() string`

GetTimeout returns the Timeout field if non-nil, zero value otherwise.

### GetTimeoutOk

`func (o *HealthCheckRequest) GetTimeoutOk() (*string, bool)`

GetTimeoutOk returns a tuple with the Timeout field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetTimeout

`func (o *HealthCheckRequest) SetTimeout(v string)`

SetTimeout sets Timeout field to given value.

### HasTimeout

`func (o *HealthCheckRequest) HasTimeout() bool`

HasTimeout returns a boolean if a field has been set.

### SetTimeoutNil

`func (o *HealthCheckRequest) SetTimeoutNil(b bool)`

 SetTimeoutNil sets the value for Timeout to be an explicit nil

### UnsetTimeout
`func (o *HealthCheckRequest) UnsetTimeout()`

UnsetTimeout ensures that no value is present for Timeout, not even an explicit nil
### GetType

`func (o *HealthCheckRequest) GetType() string`

GetType returns the Type field if non-nil, zero value otherwise.

### GetTypeOk

`func (o *HealthCheckRequest) GetTypeOk() (*string, bool)`

GetTypeOk returns a tuple with the Type field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetType

`func (o *HealthCheckRequest) SetType(v string)`

SetType sets Type field to given value.


### GetUrl

`func (o *HealthCheckRequest) GetUrl() string`

GetUrl returns the Url field if non-nil, zero value otherwise.

### GetUrlOk

`func (o *HealthCheckRequest) GetUrlOk() (*string, bool)`

GetUrlOk returns a tuple with the Url field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetUrl

`func (o *HealthCheckRequest) SetUrl(v string)`

SetUrl sets Url field to given value.

### HasUrl

`func (o *HealthCheckRequest) HasUrl() bool`

HasUrl returns a boolean if a field has been set.

### SetUrlNil

`func (o *HealthCheckRequest) SetUrlNil(b bool)`

 SetUrlNil sets the value for Url to be an explicit nil

### UnsetUrl
`func (o *HealthCheckRequest) UnsetUrl()`

UnsetUrl ensures that no value is present for Url, not even an explicit nil

[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


