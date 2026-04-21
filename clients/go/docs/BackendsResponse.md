# BackendsResponse

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Groups** | [**[]BackendGroupInfo**](BackendGroupInfo.md) | Backend group details. | 
**TotalGroups** | **int32** | Total number of backend groups. | 

## Methods

### NewBackendsResponse

`func NewBackendsResponse(groups []BackendGroupInfo, totalGroups int32, ) *BackendsResponse`

NewBackendsResponse instantiates a new BackendsResponse object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewBackendsResponseWithDefaults

`func NewBackendsResponseWithDefaults() *BackendsResponse`

NewBackendsResponseWithDefaults instantiates a new BackendsResponse object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetGroups

`func (o *BackendsResponse) GetGroups() []BackendGroupInfo`

GetGroups returns the Groups field if non-nil, zero value otherwise.

### GetGroupsOk

`func (o *BackendsResponse) GetGroupsOk() (*[]BackendGroupInfo, bool)`

GetGroupsOk returns a tuple with the Groups field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetGroups

`func (o *BackendsResponse) SetGroups(v []BackendGroupInfo)`

SetGroups sets Groups field to given value.


### GetTotalGroups

`func (o *BackendsResponse) GetTotalGroups() int32`

GetTotalGroups returns the TotalGroups field if non-nil, zero value otherwise.

### GetTotalGroupsOk

`func (o *BackendsResponse) GetTotalGroupsOk() (*int32, bool)`

GetTotalGroupsOk returns a tuple with the TotalGroups field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetTotalGroups

`func (o *BackendsResponse) SetTotalGroups(v int32)`

SetTotalGroups sets TotalGroups field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


