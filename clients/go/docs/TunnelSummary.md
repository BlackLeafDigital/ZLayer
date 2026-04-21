# TunnelSummary

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**CreatedAt** | **int64** | When the tunnel was created (Unix timestamp) | 
**ExpiresAt** | **int64** | When the token expires (Unix timestamp) | 
**Id** | **string** | Unique tunnel identifier | 
**LastConnected** | Pointer to **NullableInt64** | Last time the tunnel connected (Unix timestamp, if ever) | [optional] 
**Name** | **string** | Name of the tunnel | 
**Services** | **[]string** | Services this tunnel can expose | 
**Status** | **string** | Current status (active, disconnected, expired) | 

## Methods

### NewTunnelSummary

`func NewTunnelSummary(createdAt int64, expiresAt int64, id string, name string, services []string, status string, ) *TunnelSummary`

NewTunnelSummary instantiates a new TunnelSummary object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewTunnelSummaryWithDefaults

`func NewTunnelSummaryWithDefaults() *TunnelSummary`

NewTunnelSummaryWithDefaults instantiates a new TunnelSummary object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetCreatedAt

`func (o *TunnelSummary) GetCreatedAt() int64`

GetCreatedAt returns the CreatedAt field if non-nil, zero value otherwise.

### GetCreatedAtOk

`func (o *TunnelSummary) GetCreatedAtOk() (*int64, bool)`

GetCreatedAtOk returns a tuple with the CreatedAt field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetCreatedAt

`func (o *TunnelSummary) SetCreatedAt(v int64)`

SetCreatedAt sets CreatedAt field to given value.


### GetExpiresAt

`func (o *TunnelSummary) GetExpiresAt() int64`

GetExpiresAt returns the ExpiresAt field if non-nil, zero value otherwise.

### GetExpiresAtOk

`func (o *TunnelSummary) GetExpiresAtOk() (*int64, bool)`

GetExpiresAtOk returns a tuple with the ExpiresAt field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetExpiresAt

`func (o *TunnelSummary) SetExpiresAt(v int64)`

SetExpiresAt sets ExpiresAt field to given value.


### GetId

`func (o *TunnelSummary) GetId() string`

GetId returns the Id field if non-nil, zero value otherwise.

### GetIdOk

`func (o *TunnelSummary) GetIdOk() (*string, bool)`

GetIdOk returns a tuple with the Id field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetId

`func (o *TunnelSummary) SetId(v string)`

SetId sets Id field to given value.


### GetLastConnected

`func (o *TunnelSummary) GetLastConnected() int64`

GetLastConnected returns the LastConnected field if non-nil, zero value otherwise.

### GetLastConnectedOk

`func (o *TunnelSummary) GetLastConnectedOk() (*int64, bool)`

GetLastConnectedOk returns a tuple with the LastConnected field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetLastConnected

`func (o *TunnelSummary) SetLastConnected(v int64)`

SetLastConnected sets LastConnected field to given value.

### HasLastConnected

`func (o *TunnelSummary) HasLastConnected() bool`

HasLastConnected returns a boolean if a field has been set.

### SetLastConnectedNil

`func (o *TunnelSummary) SetLastConnectedNil(b bool)`

 SetLastConnectedNil sets the value for LastConnected to be an explicit nil

### UnsetLastConnected
`func (o *TunnelSummary) UnsetLastConnected()`

UnsetLastConnected ensures that no value is present for LastConnected, not even an explicit nil
### GetName

`func (o *TunnelSummary) GetName() string`

GetName returns the Name field if non-nil, zero value otherwise.

### GetNameOk

`func (o *TunnelSummary) GetNameOk() (*string, bool)`

GetNameOk returns a tuple with the Name field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetName

`func (o *TunnelSummary) SetName(v string)`

SetName sets Name field to given value.


### GetServices

`func (o *TunnelSummary) GetServices() []string`

GetServices returns the Services field if non-nil, zero value otherwise.

### GetServicesOk

`func (o *TunnelSummary) GetServicesOk() (*[]string, bool)`

GetServicesOk returns a tuple with the Services field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetServices

`func (o *TunnelSummary) SetServices(v []string)`

SetServices sets Services field to given value.


### GetStatus

`func (o *TunnelSummary) GetStatus() string`

GetStatus returns the Status field if non-nil, zero value otherwise.

### GetStatusOk

`func (o *TunnelSummary) GetStatusOk() (*string, bool)`

GetStatusOk returns a tuple with the Status field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetStatus

`func (o *TunnelSummary) SetStatus(v string)`

SetStatus sets Status field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


