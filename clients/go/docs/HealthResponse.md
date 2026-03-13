# HealthResponse

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**RuntimeName** | Pointer to **string** | Container runtime name (e.g. \&quot;youki\&quot;, \&quot;mac-sandbox\&quot;, \&quot;docker\&quot;) | [optional] 
**Status** | **string** | Service status | 
**UptimeSecs** | Pointer to **NullableInt64** | Uptime in seconds | [optional] 
**Version** | **string** | Service version | 

## Methods

### NewHealthResponse

`func NewHealthResponse(status string, version string, ) *HealthResponse`

NewHealthResponse instantiates a new HealthResponse object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewHealthResponseWithDefaults

`func NewHealthResponseWithDefaults() *HealthResponse`

NewHealthResponseWithDefaults instantiates a new HealthResponse object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetRuntimeName

`func (o *HealthResponse) GetRuntimeName() string`

GetRuntimeName returns the RuntimeName field if non-nil, zero value otherwise.

### GetRuntimeNameOk

`func (o *HealthResponse) GetRuntimeNameOk() (*string, bool)`

GetRuntimeNameOk returns a tuple with the RuntimeName field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetRuntimeName

`func (o *HealthResponse) SetRuntimeName(v string)`

SetRuntimeName sets RuntimeName field to given value.

### HasRuntimeName

`func (o *HealthResponse) HasRuntimeName() bool`

HasRuntimeName returns a boolean if a field has been set.

### GetStatus

`func (o *HealthResponse) GetStatus() string`

GetStatus returns the Status field if non-nil, zero value otherwise.

### GetStatusOk

`func (o *HealthResponse) GetStatusOk() (*string, bool)`

GetStatusOk returns a tuple with the Status field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetStatus

`func (o *HealthResponse) SetStatus(v string)`

SetStatus sets Status field to given value.


### GetUptimeSecs

`func (o *HealthResponse) GetUptimeSecs() int64`

GetUptimeSecs returns the UptimeSecs field if non-nil, zero value otherwise.

### GetUptimeSecsOk

`func (o *HealthResponse) GetUptimeSecsOk() (*int64, bool)`

GetUptimeSecsOk returns a tuple with the UptimeSecs field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetUptimeSecs

`func (o *HealthResponse) SetUptimeSecs(v int64)`

SetUptimeSecs sets UptimeSecs field to given value.

### HasUptimeSecs

`func (o *HealthResponse) HasUptimeSecs() bool`

HasUptimeSecs returns a boolean if a field has been set.

### SetUptimeSecsNil

`func (o *HealthResponse) SetUptimeSecsNil(b bool)`

 SetUptimeSecsNil sets the value for UptimeSecs to be an explicit nil

### UnsetUptimeSecs
`func (o *HealthResponse) UnsetUptimeSecs()`

UnsetUptimeSecs ensures that no value is present for UptimeSecs, not even an explicit nil
### GetVersion

`func (o *HealthResponse) GetVersion() string`

GetVersion returns the Version field if non-nil, zero value otherwise.

### GetVersionOk

`func (o *HealthResponse) GetVersionOk() (*string, bool)`

GetVersionOk returns a tuple with the Version field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetVersion

`func (o *HealthResponse) SetVersion(v string)`

SetVersion sets Version field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


