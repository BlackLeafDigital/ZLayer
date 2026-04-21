# StoredSync

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**AutoApply** | Pointer to **bool** | Whether the sync should automatically apply on pull. | [optional] 
**CreatedAt** | **string** | When the sync was created. | 
**DeleteMissing** | Pointer to **bool** | Whether the sync apply should delete resources that are present on the API but missing from the manifest directory. Defaults to &#x60;false&#x60; — the safer behaviour, which skips deletes and only creates/updates. | [optional] 
**GitPath** | **string** | Path within the project&#39;s checkout to scan for resource YAMLs. | 
**Id** | **string** | UUID identifier. | 
**LastAppliedSha** | Pointer to **NullableString** | The commit SHA at which this sync was last applied. | [optional] 
**Name** | **string** | Display name for this sync. | 
**ProjectId** | Pointer to **NullableString** | Linked project id (the git checkout to scan). | [optional] 
**UpdatedAt** | **string** | When the sync was last updated. | 

## Methods

### NewStoredSync

`func NewStoredSync(createdAt string, gitPath string, id string, name string, updatedAt string, ) *StoredSync`

NewStoredSync instantiates a new StoredSync object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewStoredSyncWithDefaults

`func NewStoredSyncWithDefaults() *StoredSync`

NewStoredSyncWithDefaults instantiates a new StoredSync object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetAutoApply

`func (o *StoredSync) GetAutoApply() bool`

GetAutoApply returns the AutoApply field if non-nil, zero value otherwise.

### GetAutoApplyOk

`func (o *StoredSync) GetAutoApplyOk() (*bool, bool)`

GetAutoApplyOk returns a tuple with the AutoApply field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetAutoApply

`func (o *StoredSync) SetAutoApply(v bool)`

SetAutoApply sets AutoApply field to given value.

### HasAutoApply

`func (o *StoredSync) HasAutoApply() bool`

HasAutoApply returns a boolean if a field has been set.

### GetCreatedAt

`func (o *StoredSync) GetCreatedAt() string`

GetCreatedAt returns the CreatedAt field if non-nil, zero value otherwise.

### GetCreatedAtOk

`func (o *StoredSync) GetCreatedAtOk() (*string, bool)`

GetCreatedAtOk returns a tuple with the CreatedAt field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetCreatedAt

`func (o *StoredSync) SetCreatedAt(v string)`

SetCreatedAt sets CreatedAt field to given value.


### GetDeleteMissing

`func (o *StoredSync) GetDeleteMissing() bool`

GetDeleteMissing returns the DeleteMissing field if non-nil, zero value otherwise.

### GetDeleteMissingOk

`func (o *StoredSync) GetDeleteMissingOk() (*bool, bool)`

GetDeleteMissingOk returns a tuple with the DeleteMissing field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetDeleteMissing

`func (o *StoredSync) SetDeleteMissing(v bool)`

SetDeleteMissing sets DeleteMissing field to given value.

### HasDeleteMissing

`func (o *StoredSync) HasDeleteMissing() bool`

HasDeleteMissing returns a boolean if a field has been set.

### GetGitPath

`func (o *StoredSync) GetGitPath() string`

GetGitPath returns the GitPath field if non-nil, zero value otherwise.

### GetGitPathOk

`func (o *StoredSync) GetGitPathOk() (*string, bool)`

GetGitPathOk returns a tuple with the GitPath field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetGitPath

`func (o *StoredSync) SetGitPath(v string)`

SetGitPath sets GitPath field to given value.


### GetId

`func (o *StoredSync) GetId() string`

GetId returns the Id field if non-nil, zero value otherwise.

### GetIdOk

`func (o *StoredSync) GetIdOk() (*string, bool)`

GetIdOk returns a tuple with the Id field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetId

`func (o *StoredSync) SetId(v string)`

SetId sets Id field to given value.


### GetLastAppliedSha

`func (o *StoredSync) GetLastAppliedSha() string`

GetLastAppliedSha returns the LastAppliedSha field if non-nil, zero value otherwise.

### GetLastAppliedShaOk

`func (o *StoredSync) GetLastAppliedShaOk() (*string, bool)`

GetLastAppliedShaOk returns a tuple with the LastAppliedSha field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetLastAppliedSha

`func (o *StoredSync) SetLastAppliedSha(v string)`

SetLastAppliedSha sets LastAppliedSha field to given value.

### HasLastAppliedSha

`func (o *StoredSync) HasLastAppliedSha() bool`

HasLastAppliedSha returns a boolean if a field has been set.

### SetLastAppliedShaNil

`func (o *StoredSync) SetLastAppliedShaNil(b bool)`

 SetLastAppliedShaNil sets the value for LastAppliedSha to be an explicit nil

### UnsetLastAppliedSha
`func (o *StoredSync) UnsetLastAppliedSha()`

UnsetLastAppliedSha ensures that no value is present for LastAppliedSha, not even an explicit nil
### GetName

`func (o *StoredSync) GetName() string`

GetName returns the Name field if non-nil, zero value otherwise.

### GetNameOk

`func (o *StoredSync) GetNameOk() (*string, bool)`

GetNameOk returns a tuple with the Name field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetName

`func (o *StoredSync) SetName(v string)`

SetName sets Name field to given value.


### GetProjectId

`func (o *StoredSync) GetProjectId() string`

GetProjectId returns the ProjectId field if non-nil, zero value otherwise.

### GetProjectIdOk

`func (o *StoredSync) GetProjectIdOk() (*string, bool)`

GetProjectIdOk returns a tuple with the ProjectId field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetProjectId

`func (o *StoredSync) SetProjectId(v string)`

SetProjectId sets ProjectId field to given value.

### HasProjectId

`func (o *StoredSync) HasProjectId() bool`

HasProjectId returns a boolean if a field has been set.

### SetProjectIdNil

`func (o *StoredSync) SetProjectIdNil(b bool)`

 SetProjectIdNil sets the value for ProjectId to be an explicit nil

### UnsetProjectId
`func (o *StoredSync) UnsetProjectId()`

UnsetProjectId ensures that no value is present for ProjectId, not even an explicit nil
### GetUpdatedAt

`func (o *StoredSync) GetUpdatedAt() string`

GetUpdatedAt returns the UpdatedAt field if non-nil, zero value otherwise.

### GetUpdatedAtOk

`func (o *StoredSync) GetUpdatedAtOk() (*string, bool)`

GetUpdatedAtOk returns a tuple with the UpdatedAt field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetUpdatedAt

`func (o *StoredSync) SetUpdatedAt(v string)`

SetUpdatedAt sets UpdatedAt field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


