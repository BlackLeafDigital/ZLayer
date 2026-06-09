# StoredDeployment

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**CreatedAt** | **string** | When the deployment was created | 
**Name** | **string** | Deployment name (unique identifier) | 
**Spec** | **map[string]interface{}** | The deployment specification (complex nested structure, see spec docs) | 
**Status** | [**DeploymentStatus**](DeploymentStatus.md) | Current deployment status | 
**UpdatedAt** | **string** | When the deployment was last updated | 

## Methods

### NewStoredDeployment

`func NewStoredDeployment(createdAt string, name string, spec map[string]interface{}, status DeploymentStatus, updatedAt string, ) *StoredDeployment`

NewStoredDeployment instantiates a new StoredDeployment object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewStoredDeploymentWithDefaults

`func NewStoredDeploymentWithDefaults() *StoredDeployment`

NewStoredDeploymentWithDefaults instantiates a new StoredDeployment object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetCreatedAt

`func (o *StoredDeployment) GetCreatedAt() string`

GetCreatedAt returns the CreatedAt field if non-nil, zero value otherwise.

### GetCreatedAtOk

`func (o *StoredDeployment) GetCreatedAtOk() (*string, bool)`

GetCreatedAtOk returns a tuple with the CreatedAt field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetCreatedAt

`func (o *StoredDeployment) SetCreatedAt(v string)`

SetCreatedAt sets CreatedAt field to given value.


### GetName

`func (o *StoredDeployment) GetName() string`

GetName returns the Name field if non-nil, zero value otherwise.

### GetNameOk

`func (o *StoredDeployment) GetNameOk() (*string, bool)`

GetNameOk returns a tuple with the Name field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetName

`func (o *StoredDeployment) SetName(v string)`

SetName sets Name field to given value.


### GetSpec

`func (o *StoredDeployment) GetSpec() map[string]interface{}`

GetSpec returns the Spec field if non-nil, zero value otherwise.

### GetSpecOk

`func (o *StoredDeployment) GetSpecOk() (*map[string]interface{}, bool)`

GetSpecOk returns a tuple with the Spec field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetSpec

`func (o *StoredDeployment) SetSpec(v map[string]interface{})`

SetSpec sets Spec field to given value.


### GetStatus

`func (o *StoredDeployment) GetStatus() DeploymentStatus`

GetStatus returns the Status field if non-nil, zero value otherwise.

### GetStatusOk

`func (o *StoredDeployment) GetStatusOk() (*DeploymentStatus, bool)`

GetStatusOk returns a tuple with the Status field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetStatus

`func (o *StoredDeployment) SetStatus(v DeploymentStatus)`

SetStatus sets Status field to given value.


### GetUpdatedAt

`func (o *StoredDeployment) GetUpdatedAt() string`

GetUpdatedAt returns the UpdatedAt field if non-nil, zero value otherwise.

### GetUpdatedAtOk

`func (o *StoredDeployment) GetUpdatedAtOk() (*string, bool)`

GetUpdatedAtOk returns a tuple with the UpdatedAt field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetUpdatedAt

`func (o *StoredDeployment) SetUpdatedAt(v string)`

SetUpdatedAt sets UpdatedAt field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


