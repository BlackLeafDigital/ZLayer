# PruneResultDto

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Deleted** | **[]string** | Image references or digests that were removed. | 
**SpaceReclaimed** | **int64** | Bytes reclaimed from the cache. | 

## Methods

### NewPruneResultDto

`func NewPruneResultDto(deleted []string, spaceReclaimed int64, ) *PruneResultDto`

NewPruneResultDto instantiates a new PruneResultDto object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewPruneResultDtoWithDefaults

`func NewPruneResultDtoWithDefaults() *PruneResultDto`

NewPruneResultDtoWithDefaults instantiates a new PruneResultDto object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetDeleted

`func (o *PruneResultDto) GetDeleted() []string`

GetDeleted returns the Deleted field if non-nil, zero value otherwise.

### GetDeletedOk

`func (o *PruneResultDto) GetDeletedOk() (*[]string, bool)`

GetDeletedOk returns a tuple with the Deleted field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetDeleted

`func (o *PruneResultDto) SetDeleted(v []string)`

SetDeleted sets Deleted field to given value.


### GetSpaceReclaimed

`func (o *PruneResultDto) GetSpaceReclaimed() int64`

GetSpaceReclaimed returns the SpaceReclaimed field if non-nil, zero value otherwise.

### GetSpaceReclaimedOk

`func (o *PruneResultDto) GetSpaceReclaimedOk() (*int64, bool)`

GetSpaceReclaimedOk returns a tuple with the SpaceReclaimed field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetSpaceReclaimed

`func (o *PruneResultDto) SetSpaceReclaimed(v int64)`

SetSpaceReclaimed sets SpaceReclaimed field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


