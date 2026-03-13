# DeploymentSummary

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**CreatedAt** | **string** | Created timestamp | 
**Name** | **string** | Deployment name | 
**ServiceCount** | **int32** | Number of services | 
**Status** | **string** | Deployment status | 

## Methods

### NewDeploymentSummary

`func NewDeploymentSummary(createdAt string, name string, serviceCount int32, status string, ) *DeploymentSummary`

NewDeploymentSummary instantiates a new DeploymentSummary object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewDeploymentSummaryWithDefaults

`func NewDeploymentSummaryWithDefaults() *DeploymentSummary`

NewDeploymentSummaryWithDefaults instantiates a new DeploymentSummary object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetCreatedAt

`func (o *DeploymentSummary) GetCreatedAt() string`

GetCreatedAt returns the CreatedAt field if non-nil, zero value otherwise.

### GetCreatedAtOk

`func (o *DeploymentSummary) GetCreatedAtOk() (*string, bool)`

GetCreatedAtOk returns a tuple with the CreatedAt field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetCreatedAt

`func (o *DeploymentSummary) SetCreatedAt(v string)`

SetCreatedAt sets CreatedAt field to given value.


### GetName

`func (o *DeploymentSummary) GetName() string`

GetName returns the Name field if non-nil, zero value otherwise.

### GetNameOk

`func (o *DeploymentSummary) GetNameOk() (*string, bool)`

GetNameOk returns a tuple with the Name field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetName

`func (o *DeploymentSummary) SetName(v string)`

SetName sets Name field to given value.


### GetServiceCount

`func (o *DeploymentSummary) GetServiceCount() int32`

GetServiceCount returns the ServiceCount field if non-nil, zero value otherwise.

### GetServiceCountOk

`func (o *DeploymentSummary) GetServiceCountOk() (*int32, bool)`

GetServiceCountOk returns a tuple with the ServiceCount field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetServiceCount

`func (o *DeploymentSummary) SetServiceCount(v int32)`

SetServiceCount sets ServiceCount field to given value.


### GetStatus

`func (o *DeploymentSummary) GetStatus() string`

GetStatus returns the Status field if non-nil, zero value otherwise.

### GetStatusOk

`func (o *DeploymentSummary) GetStatusOk() (*string, bool)`

GetStatusOk returns a tuple with the Status field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetStatus

`func (o *DeploymentSummary) SetStatus(v string)`

SetStatus sets Status field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


