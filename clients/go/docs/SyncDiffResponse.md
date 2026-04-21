# SyncDiffResponse

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**ToCreate** | [**[]SyncResourceResponse**](SyncResourceResponse.md) | Resources to create. | 
**ToDelete** | **[]string** | Resource names to delete. | 
**ToUpdate** | [**[]SyncResourceResponse**](SyncResourceResponse.md) | Resources to update. | 

## Methods

### NewSyncDiffResponse

`func NewSyncDiffResponse(toCreate []SyncResourceResponse, toDelete []string, toUpdate []SyncResourceResponse, ) *SyncDiffResponse`

NewSyncDiffResponse instantiates a new SyncDiffResponse object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewSyncDiffResponseWithDefaults

`func NewSyncDiffResponseWithDefaults() *SyncDiffResponse`

NewSyncDiffResponseWithDefaults instantiates a new SyncDiffResponse object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetToCreate

`func (o *SyncDiffResponse) GetToCreate() []SyncResourceResponse`

GetToCreate returns the ToCreate field if non-nil, zero value otherwise.

### GetToCreateOk

`func (o *SyncDiffResponse) GetToCreateOk() (*[]SyncResourceResponse, bool)`

GetToCreateOk returns a tuple with the ToCreate field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetToCreate

`func (o *SyncDiffResponse) SetToCreate(v []SyncResourceResponse)`

SetToCreate sets ToCreate field to given value.


### GetToDelete

`func (o *SyncDiffResponse) GetToDelete() []string`

GetToDelete returns the ToDelete field if non-nil, zero value otherwise.

### GetToDeleteOk

`func (o *SyncDiffResponse) GetToDeleteOk() (*[]string, bool)`

GetToDeleteOk returns a tuple with the ToDelete field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetToDelete

`func (o *SyncDiffResponse) SetToDelete(v []string)`

SetToDelete sets ToDelete field to given value.


### GetToUpdate

`func (o *SyncDiffResponse) GetToUpdate() []SyncResourceResponse`

GetToUpdate returns the ToUpdate field if non-nil, zero value otherwise.

### GetToUpdateOk

`func (o *SyncDiffResponse) GetToUpdateOk() (*[]SyncResourceResponse, bool)`

GetToUpdateOk returns a tuple with the ToUpdate field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetToUpdate

`func (o *SyncDiffResponse) SetToUpdate(v []SyncResourceResponse)`

SetToUpdate sets ToUpdate field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


