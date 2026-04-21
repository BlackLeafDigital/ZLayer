# UpdateProjectRequest

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**AutoDeploy** | Pointer to **NullableBool** | Enable or disable automatic deploy on new commits. | [optional] 
**BuildKind** | Pointer to [**NullableBuildKind**](BuildKind.md) | New build kind. | [optional] 
**BuildPath** | Pointer to **NullableString** | New build path. | [optional] 
**DefaultEnvironmentId** | Pointer to **NullableString** | New default environment id. | [optional] 
**DeploySpecPath** | Pointer to **NullableString** | New path (inside the cloned repo) to the &#x60;DeploymentSpec&#x60; YAML that workflow &#x60;DeployProject&#x60; actions should apply. Pass &#x60;\&quot;\&quot;&#x60; to clear. | [optional] 
**Description** | Pointer to **NullableString** | New description. | [optional] 
**GitBranch** | Pointer to **NullableString** | New git branch. | [optional] 
**GitCredentialId** | Pointer to **NullableString** | New git credential id. | [optional] 
**GitUrl** | Pointer to **NullableString** | New git URL. | [optional] 
**Name** | Pointer to **NullableString** | New project name. | [optional] 
**PollIntervalSecs** | Pointer to **NullableInt64** | Set polling interval in seconds. &#x60;null&#x60; disables polling. | [optional] 
**RegistryCredentialId** | Pointer to **NullableString** | New registry credential id. | [optional] 

## Methods

### NewUpdateProjectRequest

`func NewUpdateProjectRequest() *UpdateProjectRequest`

NewUpdateProjectRequest instantiates a new UpdateProjectRequest object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewUpdateProjectRequestWithDefaults

`func NewUpdateProjectRequestWithDefaults() *UpdateProjectRequest`

NewUpdateProjectRequestWithDefaults instantiates a new UpdateProjectRequest object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetAutoDeploy

`func (o *UpdateProjectRequest) GetAutoDeploy() bool`

GetAutoDeploy returns the AutoDeploy field if non-nil, zero value otherwise.

### GetAutoDeployOk

`func (o *UpdateProjectRequest) GetAutoDeployOk() (*bool, bool)`

GetAutoDeployOk returns a tuple with the AutoDeploy field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetAutoDeploy

`func (o *UpdateProjectRequest) SetAutoDeploy(v bool)`

SetAutoDeploy sets AutoDeploy field to given value.

### HasAutoDeploy

`func (o *UpdateProjectRequest) HasAutoDeploy() bool`

HasAutoDeploy returns a boolean if a field has been set.

### SetAutoDeployNil

`func (o *UpdateProjectRequest) SetAutoDeployNil(b bool)`

 SetAutoDeployNil sets the value for AutoDeploy to be an explicit nil

### UnsetAutoDeploy
`func (o *UpdateProjectRequest) UnsetAutoDeploy()`

UnsetAutoDeploy ensures that no value is present for AutoDeploy, not even an explicit nil
### GetBuildKind

`func (o *UpdateProjectRequest) GetBuildKind() BuildKind`

GetBuildKind returns the BuildKind field if non-nil, zero value otherwise.

### GetBuildKindOk

`func (o *UpdateProjectRequest) GetBuildKindOk() (*BuildKind, bool)`

GetBuildKindOk returns a tuple with the BuildKind field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetBuildKind

`func (o *UpdateProjectRequest) SetBuildKind(v BuildKind)`

SetBuildKind sets BuildKind field to given value.

### HasBuildKind

`func (o *UpdateProjectRequest) HasBuildKind() bool`

HasBuildKind returns a boolean if a field has been set.

### SetBuildKindNil

`func (o *UpdateProjectRequest) SetBuildKindNil(b bool)`

 SetBuildKindNil sets the value for BuildKind to be an explicit nil

### UnsetBuildKind
`func (o *UpdateProjectRequest) UnsetBuildKind()`

UnsetBuildKind ensures that no value is present for BuildKind, not even an explicit nil
### GetBuildPath

`func (o *UpdateProjectRequest) GetBuildPath() string`

GetBuildPath returns the BuildPath field if non-nil, zero value otherwise.

### GetBuildPathOk

`func (o *UpdateProjectRequest) GetBuildPathOk() (*string, bool)`

GetBuildPathOk returns a tuple with the BuildPath field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetBuildPath

`func (o *UpdateProjectRequest) SetBuildPath(v string)`

SetBuildPath sets BuildPath field to given value.

### HasBuildPath

`func (o *UpdateProjectRequest) HasBuildPath() bool`

HasBuildPath returns a boolean if a field has been set.

### SetBuildPathNil

`func (o *UpdateProjectRequest) SetBuildPathNil(b bool)`

 SetBuildPathNil sets the value for BuildPath to be an explicit nil

### UnsetBuildPath
`func (o *UpdateProjectRequest) UnsetBuildPath()`

UnsetBuildPath ensures that no value is present for BuildPath, not even an explicit nil
### GetDefaultEnvironmentId

`func (o *UpdateProjectRequest) GetDefaultEnvironmentId() string`

GetDefaultEnvironmentId returns the DefaultEnvironmentId field if non-nil, zero value otherwise.

### GetDefaultEnvironmentIdOk

`func (o *UpdateProjectRequest) GetDefaultEnvironmentIdOk() (*string, bool)`

GetDefaultEnvironmentIdOk returns a tuple with the DefaultEnvironmentId field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetDefaultEnvironmentId

`func (o *UpdateProjectRequest) SetDefaultEnvironmentId(v string)`

SetDefaultEnvironmentId sets DefaultEnvironmentId field to given value.

### HasDefaultEnvironmentId

`func (o *UpdateProjectRequest) HasDefaultEnvironmentId() bool`

HasDefaultEnvironmentId returns a boolean if a field has been set.

### SetDefaultEnvironmentIdNil

`func (o *UpdateProjectRequest) SetDefaultEnvironmentIdNil(b bool)`

 SetDefaultEnvironmentIdNil sets the value for DefaultEnvironmentId to be an explicit nil

### UnsetDefaultEnvironmentId
`func (o *UpdateProjectRequest) UnsetDefaultEnvironmentId()`

UnsetDefaultEnvironmentId ensures that no value is present for DefaultEnvironmentId, not even an explicit nil
### GetDeploySpecPath

`func (o *UpdateProjectRequest) GetDeploySpecPath() string`

GetDeploySpecPath returns the DeploySpecPath field if non-nil, zero value otherwise.

### GetDeploySpecPathOk

`func (o *UpdateProjectRequest) GetDeploySpecPathOk() (*string, bool)`

GetDeploySpecPathOk returns a tuple with the DeploySpecPath field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetDeploySpecPath

`func (o *UpdateProjectRequest) SetDeploySpecPath(v string)`

SetDeploySpecPath sets DeploySpecPath field to given value.

### HasDeploySpecPath

`func (o *UpdateProjectRequest) HasDeploySpecPath() bool`

HasDeploySpecPath returns a boolean if a field has been set.

### SetDeploySpecPathNil

`func (o *UpdateProjectRequest) SetDeploySpecPathNil(b bool)`

 SetDeploySpecPathNil sets the value for DeploySpecPath to be an explicit nil

### UnsetDeploySpecPath
`func (o *UpdateProjectRequest) UnsetDeploySpecPath()`

UnsetDeploySpecPath ensures that no value is present for DeploySpecPath, not even an explicit nil
### GetDescription

`func (o *UpdateProjectRequest) GetDescription() string`

GetDescription returns the Description field if non-nil, zero value otherwise.

### GetDescriptionOk

`func (o *UpdateProjectRequest) GetDescriptionOk() (*string, bool)`

GetDescriptionOk returns a tuple with the Description field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetDescription

`func (o *UpdateProjectRequest) SetDescription(v string)`

SetDescription sets Description field to given value.

### HasDescription

`func (o *UpdateProjectRequest) HasDescription() bool`

HasDescription returns a boolean if a field has been set.

### SetDescriptionNil

`func (o *UpdateProjectRequest) SetDescriptionNil(b bool)`

 SetDescriptionNil sets the value for Description to be an explicit nil

### UnsetDescription
`func (o *UpdateProjectRequest) UnsetDescription()`

UnsetDescription ensures that no value is present for Description, not even an explicit nil
### GetGitBranch

`func (o *UpdateProjectRequest) GetGitBranch() string`

GetGitBranch returns the GitBranch field if non-nil, zero value otherwise.

### GetGitBranchOk

`func (o *UpdateProjectRequest) GetGitBranchOk() (*string, bool)`

GetGitBranchOk returns a tuple with the GitBranch field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetGitBranch

`func (o *UpdateProjectRequest) SetGitBranch(v string)`

SetGitBranch sets GitBranch field to given value.

### HasGitBranch

`func (o *UpdateProjectRequest) HasGitBranch() bool`

HasGitBranch returns a boolean if a field has been set.

### SetGitBranchNil

`func (o *UpdateProjectRequest) SetGitBranchNil(b bool)`

 SetGitBranchNil sets the value for GitBranch to be an explicit nil

### UnsetGitBranch
`func (o *UpdateProjectRequest) UnsetGitBranch()`

UnsetGitBranch ensures that no value is present for GitBranch, not even an explicit nil
### GetGitCredentialId

`func (o *UpdateProjectRequest) GetGitCredentialId() string`

GetGitCredentialId returns the GitCredentialId field if non-nil, zero value otherwise.

### GetGitCredentialIdOk

`func (o *UpdateProjectRequest) GetGitCredentialIdOk() (*string, bool)`

GetGitCredentialIdOk returns a tuple with the GitCredentialId field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetGitCredentialId

`func (o *UpdateProjectRequest) SetGitCredentialId(v string)`

SetGitCredentialId sets GitCredentialId field to given value.

### HasGitCredentialId

`func (o *UpdateProjectRequest) HasGitCredentialId() bool`

HasGitCredentialId returns a boolean if a field has been set.

### SetGitCredentialIdNil

`func (o *UpdateProjectRequest) SetGitCredentialIdNil(b bool)`

 SetGitCredentialIdNil sets the value for GitCredentialId to be an explicit nil

### UnsetGitCredentialId
`func (o *UpdateProjectRequest) UnsetGitCredentialId()`

UnsetGitCredentialId ensures that no value is present for GitCredentialId, not even an explicit nil
### GetGitUrl

`func (o *UpdateProjectRequest) GetGitUrl() string`

GetGitUrl returns the GitUrl field if non-nil, zero value otherwise.

### GetGitUrlOk

`func (o *UpdateProjectRequest) GetGitUrlOk() (*string, bool)`

GetGitUrlOk returns a tuple with the GitUrl field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetGitUrl

`func (o *UpdateProjectRequest) SetGitUrl(v string)`

SetGitUrl sets GitUrl field to given value.

### HasGitUrl

`func (o *UpdateProjectRequest) HasGitUrl() bool`

HasGitUrl returns a boolean if a field has been set.

### SetGitUrlNil

`func (o *UpdateProjectRequest) SetGitUrlNil(b bool)`

 SetGitUrlNil sets the value for GitUrl to be an explicit nil

### UnsetGitUrl
`func (o *UpdateProjectRequest) UnsetGitUrl()`

UnsetGitUrl ensures that no value is present for GitUrl, not even an explicit nil
### GetName

`func (o *UpdateProjectRequest) GetName() string`

GetName returns the Name field if non-nil, zero value otherwise.

### GetNameOk

`func (o *UpdateProjectRequest) GetNameOk() (*string, bool)`

GetNameOk returns a tuple with the Name field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetName

`func (o *UpdateProjectRequest) SetName(v string)`

SetName sets Name field to given value.

### HasName

`func (o *UpdateProjectRequest) HasName() bool`

HasName returns a boolean if a field has been set.

### SetNameNil

`func (o *UpdateProjectRequest) SetNameNil(b bool)`

 SetNameNil sets the value for Name to be an explicit nil

### UnsetName
`func (o *UpdateProjectRequest) UnsetName()`

UnsetName ensures that no value is present for Name, not even an explicit nil
### GetPollIntervalSecs

`func (o *UpdateProjectRequest) GetPollIntervalSecs() int64`

GetPollIntervalSecs returns the PollIntervalSecs field if non-nil, zero value otherwise.

### GetPollIntervalSecsOk

`func (o *UpdateProjectRequest) GetPollIntervalSecsOk() (*int64, bool)`

GetPollIntervalSecsOk returns a tuple with the PollIntervalSecs field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetPollIntervalSecs

`func (o *UpdateProjectRequest) SetPollIntervalSecs(v int64)`

SetPollIntervalSecs sets PollIntervalSecs field to given value.

### HasPollIntervalSecs

`func (o *UpdateProjectRequest) HasPollIntervalSecs() bool`

HasPollIntervalSecs returns a boolean if a field has been set.

### SetPollIntervalSecsNil

`func (o *UpdateProjectRequest) SetPollIntervalSecsNil(b bool)`

 SetPollIntervalSecsNil sets the value for PollIntervalSecs to be an explicit nil

### UnsetPollIntervalSecs
`func (o *UpdateProjectRequest) UnsetPollIntervalSecs()`

UnsetPollIntervalSecs ensures that no value is present for PollIntervalSecs, not even an explicit nil
### GetRegistryCredentialId

`func (o *UpdateProjectRequest) GetRegistryCredentialId() string`

GetRegistryCredentialId returns the RegistryCredentialId field if non-nil, zero value otherwise.

### GetRegistryCredentialIdOk

`func (o *UpdateProjectRequest) GetRegistryCredentialIdOk() (*string, bool)`

GetRegistryCredentialIdOk returns a tuple with the RegistryCredentialId field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetRegistryCredentialId

`func (o *UpdateProjectRequest) SetRegistryCredentialId(v string)`

SetRegistryCredentialId sets RegistryCredentialId field to given value.

### HasRegistryCredentialId

`func (o *UpdateProjectRequest) HasRegistryCredentialId() bool`

HasRegistryCredentialId returns a boolean if a field has been set.

### SetRegistryCredentialIdNil

`func (o *UpdateProjectRequest) SetRegistryCredentialIdNil(b bool)`

 SetRegistryCredentialIdNil sets the value for RegistryCredentialId to be an explicit nil

### UnsetRegistryCredentialId
`func (o *UpdateProjectRequest) UnsetRegistryCredentialId()`

UnsetRegistryCredentialId ensures that no value is present for RegistryCredentialId, not even an explicit nil

[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


