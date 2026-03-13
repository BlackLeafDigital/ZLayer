# ServiceSummary

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Deployment** | **string** | Deployment name | 
**DesiredReplicas** | **int32** | Desired replica count | 
**Endpoints** | [**[]ServiceEndpoint**](ServiceEndpoint.md) | Service endpoints | 
**Name** | **string** | Service name | 
**Replicas** | **int32** | Current replica count | 
**Status** | **string** | Service status | 

## Methods

### NewServiceSummary

`func NewServiceSummary(deployment string, desiredReplicas int32, endpoints []ServiceEndpoint, name string, replicas int32, status string, ) *ServiceSummary`

NewServiceSummary instantiates a new ServiceSummary object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewServiceSummaryWithDefaults

`func NewServiceSummaryWithDefaults() *ServiceSummary`

NewServiceSummaryWithDefaults instantiates a new ServiceSummary object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetDeployment

`func (o *ServiceSummary) GetDeployment() string`

GetDeployment returns the Deployment field if non-nil, zero value otherwise.

### GetDeploymentOk

`func (o *ServiceSummary) GetDeploymentOk() (*string, bool)`

GetDeploymentOk returns a tuple with the Deployment field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetDeployment

`func (o *ServiceSummary) SetDeployment(v string)`

SetDeployment sets Deployment field to given value.


### GetDesiredReplicas

`func (o *ServiceSummary) GetDesiredReplicas() int32`

GetDesiredReplicas returns the DesiredReplicas field if non-nil, zero value otherwise.

### GetDesiredReplicasOk

`func (o *ServiceSummary) GetDesiredReplicasOk() (*int32, bool)`

GetDesiredReplicasOk returns a tuple with the DesiredReplicas field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetDesiredReplicas

`func (o *ServiceSummary) SetDesiredReplicas(v int32)`

SetDesiredReplicas sets DesiredReplicas field to given value.


### GetEndpoints

`func (o *ServiceSummary) GetEndpoints() []ServiceEndpoint`

GetEndpoints returns the Endpoints field if non-nil, zero value otherwise.

### GetEndpointsOk

`func (o *ServiceSummary) GetEndpointsOk() (*[]ServiceEndpoint, bool)`

GetEndpointsOk returns a tuple with the Endpoints field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetEndpoints

`func (o *ServiceSummary) SetEndpoints(v []ServiceEndpoint)`

SetEndpoints sets Endpoints field to given value.


### GetName

`func (o *ServiceSummary) GetName() string`

GetName returns the Name field if non-nil, zero value otherwise.

### GetNameOk

`func (o *ServiceSummary) GetNameOk() (*string, bool)`

GetNameOk returns a tuple with the Name field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetName

`func (o *ServiceSummary) SetName(v string)`

SetName sets Name field to given value.


### GetReplicas

`func (o *ServiceSummary) GetReplicas() int32`

GetReplicas returns the Replicas field if non-nil, zero value otherwise.

### GetReplicasOk

`func (o *ServiceSummary) GetReplicasOk() (*int32, bool)`

GetReplicasOk returns a tuple with the Replicas field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetReplicas

`func (o *ServiceSummary) SetReplicas(v int32)`

SetReplicas sets Replicas field to given value.


### GetStatus

`func (o *ServiceSummary) GetStatus() string`

GetStatus returns the Status field if non-nil, zero value otherwise.

### GetStatusOk

`func (o *ServiceSummary) GetStatusOk() (*string, bool)`

GetStatusOk returns a tuple with the Status field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetStatus

`func (o *ServiceSummary) SetStatus(v string)`

SetStatus sets Status field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


