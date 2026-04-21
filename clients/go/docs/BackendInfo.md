# BackendInfo

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**ActiveConnections** | **int64** | Number of in-flight connections. | 
**Address** | **string** | Backend address (&#x60;ip:port&#x60;). | 
**ConsecutiveFailures** | **int64** | Number of consecutive health-check failures. | 
**Healthy** | **bool** | Whether the backend is currently healthy. | 

## Methods

### NewBackendInfo

`func NewBackendInfo(activeConnections int64, address string, consecutiveFailures int64, healthy bool, ) *BackendInfo`

NewBackendInfo instantiates a new BackendInfo object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewBackendInfoWithDefaults

`func NewBackendInfoWithDefaults() *BackendInfo`

NewBackendInfoWithDefaults instantiates a new BackendInfo object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetActiveConnections

`func (o *BackendInfo) GetActiveConnections() int64`

GetActiveConnections returns the ActiveConnections field if non-nil, zero value otherwise.

### GetActiveConnectionsOk

`func (o *BackendInfo) GetActiveConnectionsOk() (*int64, bool)`

GetActiveConnectionsOk returns a tuple with the ActiveConnections field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetActiveConnections

`func (o *BackendInfo) SetActiveConnections(v int64)`

SetActiveConnections sets ActiveConnections field to given value.


### GetAddress

`func (o *BackendInfo) GetAddress() string`

GetAddress returns the Address field if non-nil, zero value otherwise.

### GetAddressOk

`func (o *BackendInfo) GetAddressOk() (*string, bool)`

GetAddressOk returns a tuple with the Address field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetAddress

`func (o *BackendInfo) SetAddress(v string)`

SetAddress sets Address field to given value.


### GetConsecutiveFailures

`func (o *BackendInfo) GetConsecutiveFailures() int64`

GetConsecutiveFailures returns the ConsecutiveFailures field if non-nil, zero value otherwise.

### GetConsecutiveFailuresOk

`func (o *BackendInfo) GetConsecutiveFailuresOk() (*int64, bool)`

GetConsecutiveFailuresOk returns a tuple with the ConsecutiveFailures field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetConsecutiveFailures

`func (o *BackendInfo) SetConsecutiveFailures(v int64)`

SetConsecutiveFailures sets ConsecutiveFailures field to given value.


### GetHealthy

`func (o *BackendInfo) GetHealthy() bool`

GetHealthy returns the Healthy field if non-nil, zero value otherwise.

### GetHealthyOk

`func (o *BackendInfo) GetHealthyOk() (*bool, bool)`

GetHealthyOk returns a tuple with the Healthy field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetHealthy

`func (o *BackendInfo) SetHealthy(v bool)`

SetHealthy sets Healthy field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


