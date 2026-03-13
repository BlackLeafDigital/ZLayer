# ServiceHealthInfo

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Endpoints** | **[]string** | Endpoint URLs for this service | 
**Health** | **string** | Health status (\&quot;healthy\&quot;, \&quot;unhealthy\&quot;, \&quot;unknown\&quot;) | 
**Name** | **string** | Service name | 
**ReplicasDesired** | **int32** | Desired replica count | 
**ReplicasRunning** | **int32** | Running replica count | 

## Methods

### NewServiceHealthInfo

`func NewServiceHealthInfo(endpoints []string, health string, name string, replicasDesired int32, replicasRunning int32, ) *ServiceHealthInfo`

NewServiceHealthInfo instantiates a new ServiceHealthInfo object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewServiceHealthInfoWithDefaults

`func NewServiceHealthInfoWithDefaults() *ServiceHealthInfo`

NewServiceHealthInfoWithDefaults instantiates a new ServiceHealthInfo object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetEndpoints

`func (o *ServiceHealthInfo) GetEndpoints() []string`

GetEndpoints returns the Endpoints field if non-nil, zero value otherwise.

### GetEndpointsOk

`func (o *ServiceHealthInfo) GetEndpointsOk() (*[]string, bool)`

GetEndpointsOk returns a tuple with the Endpoints field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetEndpoints

`func (o *ServiceHealthInfo) SetEndpoints(v []string)`

SetEndpoints sets Endpoints field to given value.


### GetHealth

`func (o *ServiceHealthInfo) GetHealth() string`

GetHealth returns the Health field if non-nil, zero value otherwise.

### GetHealthOk

`func (o *ServiceHealthInfo) GetHealthOk() (*string, bool)`

GetHealthOk returns a tuple with the Health field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetHealth

`func (o *ServiceHealthInfo) SetHealth(v string)`

SetHealth sets Health field to given value.


### GetName

`func (o *ServiceHealthInfo) GetName() string`

GetName returns the Name field if non-nil, zero value otherwise.

### GetNameOk

`func (o *ServiceHealthInfo) GetNameOk() (*string, bool)`

GetNameOk returns a tuple with the Name field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetName

`func (o *ServiceHealthInfo) SetName(v string)`

SetName sets Name field to given value.


### GetReplicasDesired

`func (o *ServiceHealthInfo) GetReplicasDesired() int32`

GetReplicasDesired returns the ReplicasDesired field if non-nil, zero value otherwise.

### GetReplicasDesiredOk

`func (o *ServiceHealthInfo) GetReplicasDesiredOk() (*int32, bool)`

GetReplicasDesiredOk returns a tuple with the ReplicasDesired field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetReplicasDesired

`func (o *ServiceHealthInfo) SetReplicasDesired(v int32)`

SetReplicasDesired sets ReplicasDesired field to given value.


### GetReplicasRunning

`func (o *ServiceHealthInfo) GetReplicasRunning() int32`

GetReplicasRunning returns the ReplicasRunning field if non-nil, zero value otherwise.

### GetReplicasRunningOk

`func (o *ServiceHealthInfo) GetReplicasRunningOk() (*int32, bool)`

GetReplicasRunningOk returns a tuple with the ReplicasRunning field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetReplicasRunning

`func (o *ServiceHealthInfo) SetReplicasRunning(v int32)`

SetReplicasRunning sets ReplicasRunning field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


