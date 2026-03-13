# DeploymentDetails

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**CreatedAt** | **string** | Created timestamp | 
**Name** | **string** | Deployment name | 
**ServiceHealth** | [**[]ServiceHealthInfo**](ServiceHealthInfo.md) | Per-service health and endpoint info | 
**Services** | **[]string** | Service names (for backwards compatibility) | 
**Status** | **string** | Deployment status | 
**UpdatedAt** | **string** | Updated timestamp | 

## Methods

### NewDeploymentDetails

`func NewDeploymentDetails(createdAt string, name string, serviceHealth []ServiceHealthInfo, services []string, status string, updatedAt string, ) *DeploymentDetails`

NewDeploymentDetails instantiates a new DeploymentDetails object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewDeploymentDetailsWithDefaults

`func NewDeploymentDetailsWithDefaults() *DeploymentDetails`

NewDeploymentDetailsWithDefaults instantiates a new DeploymentDetails object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetCreatedAt

`func (o *DeploymentDetails) GetCreatedAt() string`

GetCreatedAt returns the CreatedAt field if non-nil, zero value otherwise.

### GetCreatedAtOk

`func (o *DeploymentDetails) GetCreatedAtOk() (*string, bool)`

GetCreatedAtOk returns a tuple with the CreatedAt field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetCreatedAt

`func (o *DeploymentDetails) SetCreatedAt(v string)`

SetCreatedAt sets CreatedAt field to given value.


### GetName

`func (o *DeploymentDetails) GetName() string`

GetName returns the Name field if non-nil, zero value otherwise.

### GetNameOk

`func (o *DeploymentDetails) GetNameOk() (*string, bool)`

GetNameOk returns a tuple with the Name field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetName

`func (o *DeploymentDetails) SetName(v string)`

SetName sets Name field to given value.


### GetServiceHealth

`func (o *DeploymentDetails) GetServiceHealth() []ServiceHealthInfo`

GetServiceHealth returns the ServiceHealth field if non-nil, zero value otherwise.

### GetServiceHealthOk

`func (o *DeploymentDetails) GetServiceHealthOk() (*[]ServiceHealthInfo, bool)`

GetServiceHealthOk returns a tuple with the ServiceHealth field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetServiceHealth

`func (o *DeploymentDetails) SetServiceHealth(v []ServiceHealthInfo)`

SetServiceHealth sets ServiceHealth field to given value.


### GetServices

`func (o *DeploymentDetails) GetServices() []string`

GetServices returns the Services field if non-nil, zero value otherwise.

### GetServicesOk

`func (o *DeploymentDetails) GetServicesOk() (*[]string, bool)`

GetServicesOk returns a tuple with the Services field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetServices

`func (o *DeploymentDetails) SetServices(v []string)`

SetServices sets Services field to given value.


### GetStatus

`func (o *DeploymentDetails) GetStatus() string`

GetStatus returns the Status field if non-nil, zero value otherwise.

### GetStatusOk

`func (o *DeploymentDetails) GetStatusOk() (*string, bool)`

GetStatusOk returns a tuple with the Status field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetStatus

`func (o *DeploymentDetails) SetStatus(v string)`

SetStatus sets Status field to given value.


### GetUpdatedAt

`func (o *DeploymentDetails) GetUpdatedAt() string`

GetUpdatedAt returns the UpdatedAt field if non-nil, zero value otherwise.

### GetUpdatedAtOk

`func (o *DeploymentDetails) GetUpdatedAtOk() (*string, bool)`

GetUpdatedAtOk returns a tuple with the UpdatedAt field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetUpdatedAt

`func (o *DeploymentDetails) SetUpdatedAt(v string)`

SetUpdatedAt sets UpdatedAt field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


