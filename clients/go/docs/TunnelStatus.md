# TunnelStatus

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**ActiveConnections** | **int32** | Number of active connections | 
**AllowedServices** | **[]string** | Services this tunnel can expose | 
**ClientAddr** | Pointer to **NullableString** | Client IP address (if connected) | [optional] 
**CreatedAt** | **int64** | When the tunnel was created (Unix timestamp) | 
**ExpiresAt** | **int64** | When the token expires (Unix timestamp) | 
**Id** | **string** | Unique tunnel identifier | 
**LastConnected** | Pointer to **NullableInt64** | Last time the tunnel connected (Unix timestamp, if ever) | [optional] 
**Name** | **string** | Name of the tunnel | 
**RegisteredServices** | [**[]RegisteredServiceInfo**](RegisteredServiceInfo.md) | Currently registered services | 
**Status** | **string** | Current status (active, disconnected, expired) | 

## Methods

### NewTunnelStatus

`func NewTunnelStatus(activeConnections int32, allowedServices []string, createdAt int64, expiresAt int64, id string, name string, registeredServices []RegisteredServiceInfo, status string, ) *TunnelStatus`

NewTunnelStatus instantiates a new TunnelStatus object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewTunnelStatusWithDefaults

`func NewTunnelStatusWithDefaults() *TunnelStatus`

NewTunnelStatusWithDefaults instantiates a new TunnelStatus object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetActiveConnections

`func (o *TunnelStatus) GetActiveConnections() int32`

GetActiveConnections returns the ActiveConnections field if non-nil, zero value otherwise.

### GetActiveConnectionsOk

`func (o *TunnelStatus) GetActiveConnectionsOk() (*int32, bool)`

GetActiveConnectionsOk returns a tuple with the ActiveConnections field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetActiveConnections

`func (o *TunnelStatus) SetActiveConnections(v int32)`

SetActiveConnections sets ActiveConnections field to given value.


### GetAllowedServices

`func (o *TunnelStatus) GetAllowedServices() []string`

GetAllowedServices returns the AllowedServices field if non-nil, zero value otherwise.

### GetAllowedServicesOk

`func (o *TunnelStatus) GetAllowedServicesOk() (*[]string, bool)`

GetAllowedServicesOk returns a tuple with the AllowedServices field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetAllowedServices

`func (o *TunnelStatus) SetAllowedServices(v []string)`

SetAllowedServices sets AllowedServices field to given value.


### GetClientAddr

`func (o *TunnelStatus) GetClientAddr() string`

GetClientAddr returns the ClientAddr field if non-nil, zero value otherwise.

### GetClientAddrOk

`func (o *TunnelStatus) GetClientAddrOk() (*string, bool)`

GetClientAddrOk returns a tuple with the ClientAddr field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetClientAddr

`func (o *TunnelStatus) SetClientAddr(v string)`

SetClientAddr sets ClientAddr field to given value.

### HasClientAddr

`func (o *TunnelStatus) HasClientAddr() bool`

HasClientAddr returns a boolean if a field has been set.

### SetClientAddrNil

`func (o *TunnelStatus) SetClientAddrNil(b bool)`

 SetClientAddrNil sets the value for ClientAddr to be an explicit nil

### UnsetClientAddr
`func (o *TunnelStatus) UnsetClientAddr()`

UnsetClientAddr ensures that no value is present for ClientAddr, not even an explicit nil
### GetCreatedAt

`func (o *TunnelStatus) GetCreatedAt() int64`

GetCreatedAt returns the CreatedAt field if non-nil, zero value otherwise.

### GetCreatedAtOk

`func (o *TunnelStatus) GetCreatedAtOk() (*int64, bool)`

GetCreatedAtOk returns a tuple with the CreatedAt field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetCreatedAt

`func (o *TunnelStatus) SetCreatedAt(v int64)`

SetCreatedAt sets CreatedAt field to given value.


### GetExpiresAt

`func (o *TunnelStatus) GetExpiresAt() int64`

GetExpiresAt returns the ExpiresAt field if non-nil, zero value otherwise.

### GetExpiresAtOk

`func (o *TunnelStatus) GetExpiresAtOk() (*int64, bool)`

GetExpiresAtOk returns a tuple with the ExpiresAt field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetExpiresAt

`func (o *TunnelStatus) SetExpiresAt(v int64)`

SetExpiresAt sets ExpiresAt field to given value.


### GetId

`func (o *TunnelStatus) GetId() string`

GetId returns the Id field if non-nil, zero value otherwise.

### GetIdOk

`func (o *TunnelStatus) GetIdOk() (*string, bool)`

GetIdOk returns a tuple with the Id field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetId

`func (o *TunnelStatus) SetId(v string)`

SetId sets Id field to given value.


### GetLastConnected

`func (o *TunnelStatus) GetLastConnected() int64`

GetLastConnected returns the LastConnected field if non-nil, zero value otherwise.

### GetLastConnectedOk

`func (o *TunnelStatus) GetLastConnectedOk() (*int64, bool)`

GetLastConnectedOk returns a tuple with the LastConnected field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetLastConnected

`func (o *TunnelStatus) SetLastConnected(v int64)`

SetLastConnected sets LastConnected field to given value.

### HasLastConnected

`func (o *TunnelStatus) HasLastConnected() bool`

HasLastConnected returns a boolean if a field has been set.

### SetLastConnectedNil

`func (o *TunnelStatus) SetLastConnectedNil(b bool)`

 SetLastConnectedNil sets the value for LastConnected to be an explicit nil

### UnsetLastConnected
`func (o *TunnelStatus) UnsetLastConnected()`

UnsetLastConnected ensures that no value is present for LastConnected, not even an explicit nil
### GetName

`func (o *TunnelStatus) GetName() string`

GetName returns the Name field if non-nil, zero value otherwise.

### GetNameOk

`func (o *TunnelStatus) GetNameOk() (*string, bool)`

GetNameOk returns a tuple with the Name field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetName

`func (o *TunnelStatus) SetName(v string)`

SetName sets Name field to given value.


### GetRegisteredServices

`func (o *TunnelStatus) GetRegisteredServices() []RegisteredServiceInfo`

GetRegisteredServices returns the RegisteredServices field if non-nil, zero value otherwise.

### GetRegisteredServicesOk

`func (o *TunnelStatus) GetRegisteredServicesOk() (*[]RegisteredServiceInfo, bool)`

GetRegisteredServicesOk returns a tuple with the RegisteredServices field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetRegisteredServices

`func (o *TunnelStatus) SetRegisteredServices(v []RegisteredServiceInfo)`

SetRegisteredServices sets RegisteredServices field to given value.


### GetStatus

`func (o *TunnelStatus) GetStatus() string`

GetStatus returns the Status field if non-nil, zero value otherwise.

### GetStatusOk

`func (o *TunnelStatus) GetStatusOk() (*string, bool)`

GetStatusOk returns a tuple with the Status field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetStatus

`func (o *TunnelStatus) SetStatus(v string)`

SetStatus sets Status field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


