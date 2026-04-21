# CreateSyncRequest

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**AutoApply** | Pointer to **NullableBool** | Whether the sync should automatically apply on pull. | [optional] 
**DeleteMissing** | Pointer to **NullableBool** | Whether &#x60;apply&#x60; should delete resources on the API that are missing from the manifest directory. Defaults to &#x60;false&#x60; (the safer choice). | [optional] 
**GitPath** | **string** | Path within the project&#39;s checkout to scan for resource YAMLs. | 
**Name** | **string** | Display name for this sync. | 
**ProjectId** | Pointer to **NullableString** | Linked project id. | [optional] 

## Methods

### NewCreateSyncRequest

`func NewCreateSyncRequest(gitPath string, name string, ) *CreateSyncRequest`

NewCreateSyncRequest instantiates a new CreateSyncRequest object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewCreateSyncRequestWithDefaults

`func NewCreateSyncRequestWithDefaults() *CreateSyncRequest`

NewCreateSyncRequestWithDefaults instantiates a new CreateSyncRequest object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetAutoApply

`func (o *CreateSyncRequest) GetAutoApply() bool`

GetAutoApply returns the AutoApply field if non-nil, zero value otherwise.

### GetAutoApplyOk

`func (o *CreateSyncRequest) GetAutoApplyOk() (*bool, bool)`

GetAutoApplyOk returns a tuple with the AutoApply field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetAutoApply

`func (o *CreateSyncRequest) SetAutoApply(v bool)`

SetAutoApply sets AutoApply field to given value.

### HasAutoApply

`func (o *CreateSyncRequest) HasAutoApply() bool`

HasAutoApply returns a boolean if a field has been set.

### SetAutoApplyNil

`func (o *CreateSyncRequest) SetAutoApplyNil(b bool)`

 SetAutoApplyNil sets the value for AutoApply to be an explicit nil

### UnsetAutoApply
`func (o *CreateSyncRequest) UnsetAutoApply()`

UnsetAutoApply ensures that no value is present for AutoApply, not even an explicit nil
### GetDeleteMissing

`func (o *CreateSyncRequest) GetDeleteMissing() bool`

GetDeleteMissing returns the DeleteMissing field if non-nil, zero value otherwise.

### GetDeleteMissingOk

`func (o *CreateSyncRequest) GetDeleteMissingOk() (*bool, bool)`

GetDeleteMissingOk returns a tuple with the DeleteMissing field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetDeleteMissing

`func (o *CreateSyncRequest) SetDeleteMissing(v bool)`

SetDeleteMissing sets DeleteMissing field to given value.

### HasDeleteMissing

`func (o *CreateSyncRequest) HasDeleteMissing() bool`

HasDeleteMissing returns a boolean if a field has been set.

### SetDeleteMissingNil

`func (o *CreateSyncRequest) SetDeleteMissingNil(b bool)`

 SetDeleteMissingNil sets the value for DeleteMissing to be an explicit nil

### UnsetDeleteMissing
`func (o *CreateSyncRequest) UnsetDeleteMissing()`

UnsetDeleteMissing ensures that no value is present for DeleteMissing, not even an explicit nil
### GetGitPath

`func (o *CreateSyncRequest) GetGitPath() string`

GetGitPath returns the GitPath field if non-nil, zero value otherwise.

### GetGitPathOk

`func (o *CreateSyncRequest) GetGitPathOk() (*string, bool)`

GetGitPathOk returns a tuple with the GitPath field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetGitPath

`func (o *CreateSyncRequest) SetGitPath(v string)`

SetGitPath sets GitPath field to given value.


### GetName

`func (o *CreateSyncRequest) GetName() string`

GetName returns the Name field if non-nil, zero value otherwise.

### GetNameOk

`func (o *CreateSyncRequest) GetNameOk() (*string, bool)`

GetNameOk returns a tuple with the Name field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetName

`func (o *CreateSyncRequest) SetName(v string)`

SetName sets Name field to given value.


### GetProjectId

`func (o *CreateSyncRequest) GetProjectId() string`

GetProjectId returns the ProjectId field if non-nil, zero value otherwise.

### GetProjectIdOk

`func (o *CreateSyncRequest) GetProjectIdOk() (*string, bool)`

GetProjectIdOk returns a tuple with the ProjectId field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetProjectId

`func (o *CreateSyncRequest) SetProjectId(v string)`

SetProjectId sets ProjectId field to given value.

### HasProjectId

`func (o *CreateSyncRequest) HasProjectId() bool`

HasProjectId returns a boolean if a field has been set.

### SetProjectIdNil

`func (o *CreateSyncRequest) SetProjectIdNil(b bool)`

 SetProjectIdNil sets the value for ProjectId to be an explicit nil

### UnsetProjectId
`func (o *CreateSyncRequest) UnsetProjectId()`

UnsetProjectId ensures that no value is present for ProjectId, not even an explicit nil

[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


