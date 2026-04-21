# StoredProject

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**AutoDeploy** | Pointer to **bool** | Whether new commits on the tracked branch should automatically trigger a build + deploy cycle. | [optional] 
**BuildKind** | Pointer to [**NullableBuildKind**](BuildKind.md) | How the project is built. | [optional] 
**BuildPath** | Pointer to **NullableString** | Relative path within the repo (e.g. &#x60;\&quot;./Dockerfile\&quot;&#x60;). | [optional] 
**CreatedAt** | **string** | When the project was created. | 
**DefaultEnvironmentId** | Pointer to **NullableString** | Reference to the default environment for this project. | [optional] 
**DeploySpecPath** | Pointer to **NullableString** | Relative path (inside the cloned repo) to a &#x60;DeploymentSpec&#x60; YAML that the workflow &#x60;DeployProject&#x60; action should apply.  When &#x60;None&#x60;, the workflow &#x60;DeployProject&#x60; action fails with a clear \&quot;no deploy spec configured\&quot; error rather than silently succeeding — callers are expected to set this explicitly via &#x60;project edit&#x60;. | [optional] 
**Description** | Pointer to **NullableString** | Free-form description shown in the UI. | [optional] 
**GitBranch** | Pointer to **NullableString** | Git branch to build from (default: &#x60;\&quot;main\&quot;&#x60;). | [optional] 
**GitCredentialId** | Pointer to **NullableString** | Reference to a &#x60;GitCredential&#x60; (Phase 5.2). | [optional] 
**GitUrl** | Pointer to **NullableString** | Git repository URL (e.g. &#x60;\&quot;https://github.com/user/repo\&quot;&#x60;). | [optional] 
**Id** | **string** | UUID identifier. | 
**Name** | **string** | Project name (globally unique). | 
**OwnerId** | Pointer to **NullableString** | Reference to the owning user. | [optional] 
**PollIntervalSecs** | Pointer to **NullableInt64** | If set, the daemon polls the remote for new commits every N seconds. &#x60;None&#x60; disables polling (the project is only updated via manual pull or webhook). | [optional] 
**RegistryCredentialId** | Pointer to **NullableString** | Reference to a &#x60;RegistryCredential&#x60; (Phase 5.2). | [optional] 
**UpdatedAt** | **string** | When the project was last updated. | 

## Methods

### NewStoredProject

`func NewStoredProject(createdAt string, id string, name string, updatedAt string, ) *StoredProject`

NewStoredProject instantiates a new StoredProject object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewStoredProjectWithDefaults

`func NewStoredProjectWithDefaults() *StoredProject`

NewStoredProjectWithDefaults instantiates a new StoredProject object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetAutoDeploy

`func (o *StoredProject) GetAutoDeploy() bool`

GetAutoDeploy returns the AutoDeploy field if non-nil, zero value otherwise.

### GetAutoDeployOk

`func (o *StoredProject) GetAutoDeployOk() (*bool, bool)`

GetAutoDeployOk returns a tuple with the AutoDeploy field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetAutoDeploy

`func (o *StoredProject) SetAutoDeploy(v bool)`

SetAutoDeploy sets AutoDeploy field to given value.

### HasAutoDeploy

`func (o *StoredProject) HasAutoDeploy() bool`

HasAutoDeploy returns a boolean if a field has been set.

### GetBuildKind

`func (o *StoredProject) GetBuildKind() BuildKind`

GetBuildKind returns the BuildKind field if non-nil, zero value otherwise.

### GetBuildKindOk

`func (o *StoredProject) GetBuildKindOk() (*BuildKind, bool)`

GetBuildKindOk returns a tuple with the BuildKind field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetBuildKind

`func (o *StoredProject) SetBuildKind(v BuildKind)`

SetBuildKind sets BuildKind field to given value.

### HasBuildKind

`func (o *StoredProject) HasBuildKind() bool`

HasBuildKind returns a boolean if a field has been set.

### SetBuildKindNil

`func (o *StoredProject) SetBuildKindNil(b bool)`

 SetBuildKindNil sets the value for BuildKind to be an explicit nil

### UnsetBuildKind
`func (o *StoredProject) UnsetBuildKind()`

UnsetBuildKind ensures that no value is present for BuildKind, not even an explicit nil
### GetBuildPath

`func (o *StoredProject) GetBuildPath() string`

GetBuildPath returns the BuildPath field if non-nil, zero value otherwise.

### GetBuildPathOk

`func (o *StoredProject) GetBuildPathOk() (*string, bool)`

GetBuildPathOk returns a tuple with the BuildPath field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetBuildPath

`func (o *StoredProject) SetBuildPath(v string)`

SetBuildPath sets BuildPath field to given value.

### HasBuildPath

`func (o *StoredProject) HasBuildPath() bool`

HasBuildPath returns a boolean if a field has been set.

### SetBuildPathNil

`func (o *StoredProject) SetBuildPathNil(b bool)`

 SetBuildPathNil sets the value for BuildPath to be an explicit nil

### UnsetBuildPath
`func (o *StoredProject) UnsetBuildPath()`

UnsetBuildPath ensures that no value is present for BuildPath, not even an explicit nil
### GetCreatedAt

`func (o *StoredProject) GetCreatedAt() string`

GetCreatedAt returns the CreatedAt field if non-nil, zero value otherwise.

### GetCreatedAtOk

`func (o *StoredProject) GetCreatedAtOk() (*string, bool)`

GetCreatedAtOk returns a tuple with the CreatedAt field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetCreatedAt

`func (o *StoredProject) SetCreatedAt(v string)`

SetCreatedAt sets CreatedAt field to given value.


### GetDefaultEnvironmentId

`func (o *StoredProject) GetDefaultEnvironmentId() string`

GetDefaultEnvironmentId returns the DefaultEnvironmentId field if non-nil, zero value otherwise.

### GetDefaultEnvironmentIdOk

`func (o *StoredProject) GetDefaultEnvironmentIdOk() (*string, bool)`

GetDefaultEnvironmentIdOk returns a tuple with the DefaultEnvironmentId field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetDefaultEnvironmentId

`func (o *StoredProject) SetDefaultEnvironmentId(v string)`

SetDefaultEnvironmentId sets DefaultEnvironmentId field to given value.

### HasDefaultEnvironmentId

`func (o *StoredProject) HasDefaultEnvironmentId() bool`

HasDefaultEnvironmentId returns a boolean if a field has been set.

### SetDefaultEnvironmentIdNil

`func (o *StoredProject) SetDefaultEnvironmentIdNil(b bool)`

 SetDefaultEnvironmentIdNil sets the value for DefaultEnvironmentId to be an explicit nil

### UnsetDefaultEnvironmentId
`func (o *StoredProject) UnsetDefaultEnvironmentId()`

UnsetDefaultEnvironmentId ensures that no value is present for DefaultEnvironmentId, not even an explicit nil
### GetDeploySpecPath

`func (o *StoredProject) GetDeploySpecPath() string`

GetDeploySpecPath returns the DeploySpecPath field if non-nil, zero value otherwise.

### GetDeploySpecPathOk

`func (o *StoredProject) GetDeploySpecPathOk() (*string, bool)`

GetDeploySpecPathOk returns a tuple with the DeploySpecPath field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetDeploySpecPath

`func (o *StoredProject) SetDeploySpecPath(v string)`

SetDeploySpecPath sets DeploySpecPath field to given value.

### HasDeploySpecPath

`func (o *StoredProject) HasDeploySpecPath() bool`

HasDeploySpecPath returns a boolean if a field has been set.

### SetDeploySpecPathNil

`func (o *StoredProject) SetDeploySpecPathNil(b bool)`

 SetDeploySpecPathNil sets the value for DeploySpecPath to be an explicit nil

### UnsetDeploySpecPath
`func (o *StoredProject) UnsetDeploySpecPath()`

UnsetDeploySpecPath ensures that no value is present for DeploySpecPath, not even an explicit nil
### GetDescription

`func (o *StoredProject) GetDescription() string`

GetDescription returns the Description field if non-nil, zero value otherwise.

### GetDescriptionOk

`func (o *StoredProject) GetDescriptionOk() (*string, bool)`

GetDescriptionOk returns a tuple with the Description field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetDescription

`func (o *StoredProject) SetDescription(v string)`

SetDescription sets Description field to given value.

### HasDescription

`func (o *StoredProject) HasDescription() bool`

HasDescription returns a boolean if a field has been set.

### SetDescriptionNil

`func (o *StoredProject) SetDescriptionNil(b bool)`

 SetDescriptionNil sets the value for Description to be an explicit nil

### UnsetDescription
`func (o *StoredProject) UnsetDescription()`

UnsetDescription ensures that no value is present for Description, not even an explicit nil
### GetGitBranch

`func (o *StoredProject) GetGitBranch() string`

GetGitBranch returns the GitBranch field if non-nil, zero value otherwise.

### GetGitBranchOk

`func (o *StoredProject) GetGitBranchOk() (*string, bool)`

GetGitBranchOk returns a tuple with the GitBranch field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetGitBranch

`func (o *StoredProject) SetGitBranch(v string)`

SetGitBranch sets GitBranch field to given value.

### HasGitBranch

`func (o *StoredProject) HasGitBranch() bool`

HasGitBranch returns a boolean if a field has been set.

### SetGitBranchNil

`func (o *StoredProject) SetGitBranchNil(b bool)`

 SetGitBranchNil sets the value for GitBranch to be an explicit nil

### UnsetGitBranch
`func (o *StoredProject) UnsetGitBranch()`

UnsetGitBranch ensures that no value is present for GitBranch, not even an explicit nil
### GetGitCredentialId

`func (o *StoredProject) GetGitCredentialId() string`

GetGitCredentialId returns the GitCredentialId field if non-nil, zero value otherwise.

### GetGitCredentialIdOk

`func (o *StoredProject) GetGitCredentialIdOk() (*string, bool)`

GetGitCredentialIdOk returns a tuple with the GitCredentialId field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetGitCredentialId

`func (o *StoredProject) SetGitCredentialId(v string)`

SetGitCredentialId sets GitCredentialId field to given value.

### HasGitCredentialId

`func (o *StoredProject) HasGitCredentialId() bool`

HasGitCredentialId returns a boolean if a field has been set.

### SetGitCredentialIdNil

`func (o *StoredProject) SetGitCredentialIdNil(b bool)`

 SetGitCredentialIdNil sets the value for GitCredentialId to be an explicit nil

### UnsetGitCredentialId
`func (o *StoredProject) UnsetGitCredentialId()`

UnsetGitCredentialId ensures that no value is present for GitCredentialId, not even an explicit nil
### GetGitUrl

`func (o *StoredProject) GetGitUrl() string`

GetGitUrl returns the GitUrl field if non-nil, zero value otherwise.

### GetGitUrlOk

`func (o *StoredProject) GetGitUrlOk() (*string, bool)`

GetGitUrlOk returns a tuple with the GitUrl field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetGitUrl

`func (o *StoredProject) SetGitUrl(v string)`

SetGitUrl sets GitUrl field to given value.

### HasGitUrl

`func (o *StoredProject) HasGitUrl() bool`

HasGitUrl returns a boolean if a field has been set.

### SetGitUrlNil

`func (o *StoredProject) SetGitUrlNil(b bool)`

 SetGitUrlNil sets the value for GitUrl to be an explicit nil

### UnsetGitUrl
`func (o *StoredProject) UnsetGitUrl()`

UnsetGitUrl ensures that no value is present for GitUrl, not even an explicit nil
### GetId

`func (o *StoredProject) GetId() string`

GetId returns the Id field if non-nil, zero value otherwise.

### GetIdOk

`func (o *StoredProject) GetIdOk() (*string, bool)`

GetIdOk returns a tuple with the Id field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetId

`func (o *StoredProject) SetId(v string)`

SetId sets Id field to given value.


### GetName

`func (o *StoredProject) GetName() string`

GetName returns the Name field if non-nil, zero value otherwise.

### GetNameOk

`func (o *StoredProject) GetNameOk() (*string, bool)`

GetNameOk returns a tuple with the Name field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetName

`func (o *StoredProject) SetName(v string)`

SetName sets Name field to given value.


### GetOwnerId

`func (o *StoredProject) GetOwnerId() string`

GetOwnerId returns the OwnerId field if non-nil, zero value otherwise.

### GetOwnerIdOk

`func (o *StoredProject) GetOwnerIdOk() (*string, bool)`

GetOwnerIdOk returns a tuple with the OwnerId field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetOwnerId

`func (o *StoredProject) SetOwnerId(v string)`

SetOwnerId sets OwnerId field to given value.

### HasOwnerId

`func (o *StoredProject) HasOwnerId() bool`

HasOwnerId returns a boolean if a field has been set.

### SetOwnerIdNil

`func (o *StoredProject) SetOwnerIdNil(b bool)`

 SetOwnerIdNil sets the value for OwnerId to be an explicit nil

### UnsetOwnerId
`func (o *StoredProject) UnsetOwnerId()`

UnsetOwnerId ensures that no value is present for OwnerId, not even an explicit nil
### GetPollIntervalSecs

`func (o *StoredProject) GetPollIntervalSecs() int64`

GetPollIntervalSecs returns the PollIntervalSecs field if non-nil, zero value otherwise.

### GetPollIntervalSecsOk

`func (o *StoredProject) GetPollIntervalSecsOk() (*int64, bool)`

GetPollIntervalSecsOk returns a tuple with the PollIntervalSecs field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetPollIntervalSecs

`func (o *StoredProject) SetPollIntervalSecs(v int64)`

SetPollIntervalSecs sets PollIntervalSecs field to given value.

### HasPollIntervalSecs

`func (o *StoredProject) HasPollIntervalSecs() bool`

HasPollIntervalSecs returns a boolean if a field has been set.

### SetPollIntervalSecsNil

`func (o *StoredProject) SetPollIntervalSecsNil(b bool)`

 SetPollIntervalSecsNil sets the value for PollIntervalSecs to be an explicit nil

### UnsetPollIntervalSecs
`func (o *StoredProject) UnsetPollIntervalSecs()`

UnsetPollIntervalSecs ensures that no value is present for PollIntervalSecs, not even an explicit nil
### GetRegistryCredentialId

`func (o *StoredProject) GetRegistryCredentialId() string`

GetRegistryCredentialId returns the RegistryCredentialId field if non-nil, zero value otherwise.

### GetRegistryCredentialIdOk

`func (o *StoredProject) GetRegistryCredentialIdOk() (*string, bool)`

GetRegistryCredentialIdOk returns a tuple with the RegistryCredentialId field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetRegistryCredentialId

`func (o *StoredProject) SetRegistryCredentialId(v string)`

SetRegistryCredentialId sets RegistryCredentialId field to given value.

### HasRegistryCredentialId

`func (o *StoredProject) HasRegistryCredentialId() bool`

HasRegistryCredentialId returns a boolean if a field has been set.

### SetRegistryCredentialIdNil

`func (o *StoredProject) SetRegistryCredentialIdNil(b bool)`

 SetRegistryCredentialIdNil sets the value for RegistryCredentialId to be an explicit nil

### UnsetRegistryCredentialId
`func (o *StoredProject) UnsetRegistryCredentialId()`

UnsetRegistryCredentialId ensures that no value is present for RegistryCredentialId, not even an explicit nil
### GetUpdatedAt

`func (o *StoredProject) GetUpdatedAt() string`

GetUpdatedAt returns the UpdatedAt field if non-nil, zero value otherwise.

### GetUpdatedAtOk

`func (o *StoredProject) GetUpdatedAtOk() (*string, bool)`

GetUpdatedAtOk returns a tuple with the UpdatedAt field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetUpdatedAt

`func (o *StoredProject) SetUpdatedAt(v string)`

SetUpdatedAt sets UpdatedAt field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


