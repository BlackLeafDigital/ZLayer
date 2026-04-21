# BackendGroupInfo

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Backends** | [**[]BackendInfo**](BackendInfo.md) | Backends in this group. | 
**HealthyCount** | **int32** | Number of healthy backends. | 
**Service** | **string** | Service name. | 
**Strategy** | **string** | Load-balancing strategy (&#x60;round_robin&#x60; or &#x60;least_connections&#x60;). | 
**TotalCount** | **int32** | Total number of backends. | 

## Methods

### NewBackendGroupInfo

`func NewBackendGroupInfo(backends []BackendInfo, healthyCount int32, service string, strategy string, totalCount int32, ) *BackendGroupInfo`

NewBackendGroupInfo instantiates a new BackendGroupInfo object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewBackendGroupInfoWithDefaults

`func NewBackendGroupInfoWithDefaults() *BackendGroupInfo`

NewBackendGroupInfoWithDefaults instantiates a new BackendGroupInfo object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetBackends

`func (o *BackendGroupInfo) GetBackends() []BackendInfo`

GetBackends returns the Backends field if non-nil, zero value otherwise.

### GetBackendsOk

`func (o *BackendGroupInfo) GetBackendsOk() (*[]BackendInfo, bool)`

GetBackendsOk returns a tuple with the Backends field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetBackends

`func (o *BackendGroupInfo) SetBackends(v []BackendInfo)`

SetBackends sets Backends field to given value.


### GetHealthyCount

`func (o *BackendGroupInfo) GetHealthyCount() int32`

GetHealthyCount returns the HealthyCount field if non-nil, zero value otherwise.

### GetHealthyCountOk

`func (o *BackendGroupInfo) GetHealthyCountOk() (*int32, bool)`

GetHealthyCountOk returns a tuple with the HealthyCount field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetHealthyCount

`func (o *BackendGroupInfo) SetHealthyCount(v int32)`

SetHealthyCount sets HealthyCount field to given value.


### GetService

`func (o *BackendGroupInfo) GetService() string`

GetService returns the Service field if non-nil, zero value otherwise.

### GetServiceOk

`func (o *BackendGroupInfo) GetServiceOk() (*string, bool)`

GetServiceOk returns a tuple with the Service field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetService

`func (o *BackendGroupInfo) SetService(v string)`

SetService sets Service field to given value.


### GetStrategy

`func (o *BackendGroupInfo) GetStrategy() string`

GetStrategy returns the Strategy field if non-nil, zero value otherwise.

### GetStrategyOk

`func (o *BackendGroupInfo) GetStrategyOk() (*string, bool)`

GetStrategyOk returns a tuple with the Strategy field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetStrategy

`func (o *BackendGroupInfo) SetStrategy(v string)`

SetStrategy sets Strategy field to given value.


### GetTotalCount

`func (o *BackendGroupInfo) GetTotalCount() int32`

GetTotalCount returns the TotalCount field if non-nil, zero value otherwise.

### GetTotalCountOk

`func (o *BackendGroupInfo) GetTotalCountOk() (*int32, bool)`

GetTotalCountOk returns a tuple with the TotalCount field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetTotalCount

`func (o *BackendGroupInfo) SetTotalCount(v int32)`

SetTotalCount sets TotalCount field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


