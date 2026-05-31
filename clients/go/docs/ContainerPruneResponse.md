# ContainerPruneResponse

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**ContainersDeleted** | **[]string** | Container IDs that were removed. | 
**SpaceReclaimed** | **int64** | Bytes reclaimed from the runtime&#39;s container storage. | 

## Methods

### NewContainerPruneResponse

`func NewContainerPruneResponse(containersDeleted []string, spaceReclaimed int64, ) *ContainerPruneResponse`

NewContainerPruneResponse instantiates a new ContainerPruneResponse object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewContainerPruneResponseWithDefaults

`func NewContainerPruneResponseWithDefaults() *ContainerPruneResponse`

NewContainerPruneResponseWithDefaults instantiates a new ContainerPruneResponse object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetContainersDeleted

`func (o *ContainerPruneResponse) GetContainersDeleted() []string`

GetContainersDeleted returns the ContainersDeleted field if non-nil, zero value otherwise.

### GetContainersDeletedOk

`func (o *ContainerPruneResponse) GetContainersDeletedOk() (*[]string, bool)`

GetContainersDeletedOk returns a tuple with the ContainersDeleted field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetContainersDeleted

`func (o *ContainerPruneResponse) SetContainersDeleted(v []string)`

SetContainersDeleted sets ContainersDeleted field to given value.


### GetSpaceReclaimed

`func (o *ContainerPruneResponse) GetSpaceReclaimed() int64`

GetSpaceReclaimed returns the SpaceReclaimed field if non-nil, zero value otherwise.

### GetSpaceReclaimedOk

`func (o *ContainerPruneResponse) GetSpaceReclaimedOk() (*int64, bool)`

GetSpaceReclaimedOk returns a tuple with the SpaceReclaimed field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetSpaceReclaimed

`func (o *ContainerPruneResponse) SetSpaceReclaimed(v int64)`

SetSpaceReclaimed sets SpaceReclaimed field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


