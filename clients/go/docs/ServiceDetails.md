# ServiceDetails

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Deployment** | **string** | Deployment name | 
**DesiredReplicas** | **int32** | Desired replica count | 
**Endpoints** | [**[]ServiceEndpoint**](ServiceEndpoint.md) | Service endpoints | 
**Metrics** | [**ServiceMetrics**](ServiceMetrics.md) | Service metrics | 
**Name** | **string** | Service name | 
**Replicas** | **int32** | Current replica count | 
**Status** | **string** | Service status | 

## Methods

### NewServiceDetails

`func NewServiceDetails(deployment string, desiredReplicas int32, endpoints []ServiceEndpoint, metrics ServiceMetrics, name string, replicas int32, status string, ) *ServiceDetails`

NewServiceDetails instantiates a new ServiceDetails object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewServiceDetailsWithDefaults

`func NewServiceDetailsWithDefaults() *ServiceDetails`

NewServiceDetailsWithDefaults instantiates a new ServiceDetails object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetDeployment

`func (o *ServiceDetails) GetDeployment() string`

GetDeployment returns the Deployment field if non-nil, zero value otherwise.

### GetDeploymentOk

`func (o *ServiceDetails) GetDeploymentOk() (*string, bool)`

GetDeploymentOk returns a tuple with the Deployment field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetDeployment

`func (o *ServiceDetails) SetDeployment(v string)`

SetDeployment sets Deployment field to given value.


### GetDesiredReplicas

`func (o *ServiceDetails) GetDesiredReplicas() int32`

GetDesiredReplicas returns the DesiredReplicas field if non-nil, zero value otherwise.

### GetDesiredReplicasOk

`func (o *ServiceDetails) GetDesiredReplicasOk() (*int32, bool)`

GetDesiredReplicasOk returns a tuple with the DesiredReplicas field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetDesiredReplicas

`func (o *ServiceDetails) SetDesiredReplicas(v int32)`

SetDesiredReplicas sets DesiredReplicas field to given value.


### GetEndpoints

`func (o *ServiceDetails) GetEndpoints() []ServiceEndpoint`

GetEndpoints returns the Endpoints field if non-nil, zero value otherwise.

### GetEndpointsOk

`func (o *ServiceDetails) GetEndpointsOk() (*[]ServiceEndpoint, bool)`

GetEndpointsOk returns a tuple with the Endpoints field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetEndpoints

`func (o *ServiceDetails) SetEndpoints(v []ServiceEndpoint)`

SetEndpoints sets Endpoints field to given value.


### GetMetrics

`func (o *ServiceDetails) GetMetrics() ServiceMetrics`

GetMetrics returns the Metrics field if non-nil, zero value otherwise.

### GetMetricsOk

`func (o *ServiceDetails) GetMetricsOk() (*ServiceMetrics, bool)`

GetMetricsOk returns a tuple with the Metrics field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetMetrics

`func (o *ServiceDetails) SetMetrics(v ServiceMetrics)`

SetMetrics sets Metrics field to given value.


### GetName

`func (o *ServiceDetails) GetName() string`

GetName returns the Name field if non-nil, zero value otherwise.

### GetNameOk

`func (o *ServiceDetails) GetNameOk() (*string, bool)`

GetNameOk returns a tuple with the Name field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetName

`func (o *ServiceDetails) SetName(v string)`

SetName sets Name field to given value.


### GetReplicas

`func (o *ServiceDetails) GetReplicas() int32`

GetReplicas returns the Replicas field if non-nil, zero value otherwise.

### GetReplicasOk

`func (o *ServiceDetails) GetReplicasOk() (*int32, bool)`

GetReplicasOk returns a tuple with the Replicas field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetReplicas

`func (o *ServiceDetails) SetReplicas(v int32)`

SetReplicas sets Replicas field to given value.


### GetStatus

`func (o *ServiceDetails) GetStatus() string`

GetStatus returns the Status field if non-nil, zero value otherwise.

### GetStatusOk

`func (o *ServiceDetails) GetStatusOk() (*string, bool)`

GetStatusOk returns a tuple with the Status field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetStatus

`func (o *ServiceDetails) SetStatus(v string)`

SetStatus sets Status field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


