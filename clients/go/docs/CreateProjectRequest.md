# CreateProjectRequest

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**AutoDeploy** | Pointer to **NullableBool** | Enable automatic deploy on new commits. | [optional] 
**BuildKind** | Pointer to [**NullableBuildKind**](BuildKind.md) | How the project is built. | [optional] 
**BuildPath** | Pointer to **NullableString** | Relative build path within the repo. | [optional] 
**DefaultEnvironmentId** | Pointer to **NullableString** | Default environment id. | [optional] 
**DeploySpecPath** | Pointer to **NullableString** | Relative path (inside the cloned repo) to a &#x60;DeploymentSpec&#x60; YAML that the workflow &#x60;DeployProject&#x60; action should apply. | [optional] 
**Description** | Pointer to **NullableString** | Free-form description. | [optional] 
**GitBranch** | Pointer to **NullableString** | Git branch to build from. | [optional] 
**GitCredentialId** | Pointer to **NullableString** | Reference to a &#x60;GitCredential&#x60; id. | [optional] 
**GitUrl** | Pointer to **NullableString** | Git repository URL. | [optional] 
**Name** | **string** | Project name (globally unique). | 
**PollIntervalSecs** | Pointer to **NullableInt64** | Polling interval in seconds (None &#x3D; no polling). | [optional] 
**RegistryCredentialId** | Pointer to **NullableString** | Reference to a &#x60;RegistryCredential&#x60; id. | [optional] 

## Methods

### NewCreateProjectRequest

`func NewCreateProjectRequest(name string, ) *CreateProjectRequest`

NewCreateProjectRequest instantiates a new CreateProjectRequest object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewCreateProjectRequestWithDefaults

`func NewCreateProjectRequestWithDefaults() *CreateProjectRequest`

NewCreateProjectRequestWithDefaults instantiates a new CreateProjectRequest object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetAutoDeploy

`func (o *CreateProjectRequest) GetAutoDeploy() bool`

GetAutoDeploy returns the AutoDeploy field if non-nil, zero value otherwise.

### GetAutoDeployOk

`func (o *CreateProjectRequest) GetAutoDeployOk() (*bool, bool)`

GetAutoDeployOk returns a tuple with the AutoDeploy field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetAutoDeploy

`func (o *CreateProjectRequest) SetAutoDeploy(v bool)`

SetAutoDeploy sets AutoDeploy field to given value.

### HasAutoDeploy

`func (o *CreateProjectRequest) HasAutoDeploy() bool`

HasAutoDeploy returns a boolean if a field has been set.

### SetAutoDeployNil

`func (o *CreateProjectRequest) SetAutoDeployNil(b bool)`

 SetAutoDeployNil sets the value for AutoDeploy to be an explicit nil

### UnsetAutoDeploy
`func (o *CreateProjectRequest) UnsetAutoDeploy()`

UnsetAutoDeploy ensures that no value is present for AutoDeploy, not even an explicit nil
### GetBuildKind

`func (o *CreateProjectRequest) GetBuildKind() BuildKind`

GetBuildKind returns the BuildKind field if non-nil, zero value otherwise.

### GetBuildKindOk

`func (o *CreateProjectRequest) GetBuildKindOk() (*BuildKind, bool)`

GetBuildKindOk returns a tuple with the BuildKind field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetBuildKind

`func (o *CreateProjectRequest) SetBuildKind(v BuildKind)`

SetBuildKind sets BuildKind field to given value.

### HasBuildKind

`func (o *CreateProjectRequest) HasBuildKind() bool`

HasBuildKind returns a boolean if a field has been set.

### SetBuildKindNil

`func (o *CreateProjectRequest) SetBuildKindNil(b bool)`

 SetBuildKindNil sets the value for BuildKind to be an explicit nil

### UnsetBuildKind
`func (o *CreateProjectRequest) UnsetBuildKind()`

UnsetBuildKind ensures that no value is present for BuildKind, not even an explicit nil
### GetBuildPath

`func (o *CreateProjectRequest) GetBuildPath() string`

GetBuildPath returns the BuildPath field if non-nil, zero value otherwise.

### GetBuildPathOk

`func (o *CreateProjectRequest) GetBuildPathOk() (*string, bool)`

GetBuildPathOk returns a tuple with the BuildPath field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetBuildPath

`func (o *CreateProjectRequest) SetBuildPath(v string)`

SetBuildPath sets BuildPath field to given value.

### HasBuildPath

`func (o *CreateProjectRequest) HasBuildPath() bool`

HasBuildPath returns a boolean if a field has been set.

### SetBuildPathNil

`func (o *CreateProjectRequest) SetBuildPathNil(b bool)`

 SetBuildPathNil sets the value for BuildPath to be an explicit nil

### UnsetBuildPath
`func (o *CreateProjectRequest) UnsetBuildPath()`

UnsetBuildPath ensures that no value is present for BuildPath, not even an explicit nil
### GetDefaultEnvironmentId

`func (o *CreateProjectRequest) GetDefaultEnvironmentId() string`

GetDefaultEnvironmentId returns the DefaultEnvironmentId field if non-nil, zero value otherwise.

### GetDefaultEnvironmentIdOk

`func (o *CreateProjectRequest) GetDefaultEnvironmentIdOk() (*string, bool)`

GetDefaultEnvironmentIdOk returns a tuple with the DefaultEnvironmentId field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetDefaultEnvironmentId

`func (o *CreateProjectRequest) SetDefaultEnvironmentId(v string)`

SetDefaultEnvironmentId sets DefaultEnvironmentId field to given value.

### HasDefaultEnvironmentId

`func (o *CreateProjectRequest) HasDefaultEnvironmentId() bool`

HasDefaultEnvironmentId returns a boolean if a field has been set.

### SetDefaultEnvironmentIdNil

`func (o *CreateProjectRequest) SetDefaultEnvironmentIdNil(b bool)`

 SetDefaultEnvironmentIdNil sets the value for DefaultEnvironmentId to be an explicit nil

### UnsetDefaultEnvironmentId
`func (o *CreateProjectRequest) UnsetDefaultEnvironmentId()`

UnsetDefaultEnvironmentId ensures that no value is present for DefaultEnvironmentId, not even an explicit nil
### GetDeploySpecPath

`func (o *CreateProjectRequest) GetDeploySpecPath() string`

GetDeploySpecPath returns the DeploySpecPath field if non-nil, zero value otherwise.

### GetDeploySpecPathOk

`func (o *CreateProjectRequest) GetDeploySpecPathOk() (*string, bool)`

GetDeploySpecPathOk returns a tuple with the DeploySpecPath field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetDeploySpecPath

`func (o *CreateProjectRequest) SetDeploySpecPath(v string)`

SetDeploySpecPath sets DeploySpecPath field to given value.

### HasDeploySpecPath

`func (o *CreateProjectRequest) HasDeploySpecPath() bool`

HasDeploySpecPath returns a boolean if a field has been set.

### SetDeploySpecPathNil

`func (o *CreateProjectRequest) SetDeploySpecPathNil(b bool)`

 SetDeploySpecPathNil sets the value for DeploySpecPath to be an explicit nil

### UnsetDeploySpecPath
`func (o *CreateProjectRequest) UnsetDeploySpecPath()`

UnsetDeploySpecPath ensures that no value is present for DeploySpecPath, not even an explicit nil
### GetDescription

`func (o *CreateProjectRequest) GetDescription() string`

GetDescription returns the Description field if non-nil, zero value otherwise.

### GetDescriptionOk

`func (o *CreateProjectRequest) GetDescriptionOk() (*string, bool)`

GetDescriptionOk returns a tuple with the Description field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetDescription

`func (o *CreateProjectRequest) SetDescription(v string)`

SetDescription sets Description field to given value.

### HasDescription

`func (o *CreateProjectRequest) HasDescription() bool`

HasDescription returns a boolean if a field has been set.

### SetDescriptionNil

`func (o *CreateProjectRequest) SetDescriptionNil(b bool)`

 SetDescriptionNil sets the value for Description to be an explicit nil

### UnsetDescription
`func (o *CreateProjectRequest) UnsetDescription()`

UnsetDescription ensures that no value is present for Description, not even an explicit nil
### GetGitBranch

`func (o *CreateProjectRequest) GetGitBranch() string`

GetGitBranch returns the GitBranch field if non-nil, zero value otherwise.

### GetGitBranchOk

`func (o *CreateProjectRequest) GetGitBranchOk() (*string, bool)`

GetGitBranchOk returns a tuple with the GitBranch field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetGitBranch

`func (o *CreateProjectRequest) SetGitBranch(v string)`

SetGitBranch sets GitBranch field to given value.

### HasGitBranch

`func (o *CreateProjectRequest) HasGitBranch() bool`

HasGitBranch returns a boolean if a field has been set.

### SetGitBranchNil

`func (o *CreateProjectRequest) SetGitBranchNil(b bool)`

 SetGitBranchNil sets the value for GitBranch to be an explicit nil

### UnsetGitBranch
`func (o *CreateProjectRequest) UnsetGitBranch()`

UnsetGitBranch ensures that no value is present for GitBranch, not even an explicit nil
### GetGitCredentialId

`func (o *CreateProjectRequest) GetGitCredentialId() string`

GetGitCredentialId returns the GitCredentialId field if non-nil, zero value otherwise.

### GetGitCredentialIdOk

`func (o *CreateProjectRequest) GetGitCredentialIdOk() (*string, bool)`

GetGitCredentialIdOk returns a tuple with the GitCredentialId field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetGitCredentialId

`func (o *CreateProjectRequest) SetGitCredentialId(v string)`

SetGitCredentialId sets GitCredentialId field to given value.

### HasGitCredentialId

`func (o *CreateProjectRequest) HasGitCredentialId() bool`

HasGitCredentialId returns a boolean if a field has been set.

### SetGitCredentialIdNil

`func (o *CreateProjectRequest) SetGitCredentialIdNil(b bool)`

 SetGitCredentialIdNil sets the value for GitCredentialId to be an explicit nil

### UnsetGitCredentialId
`func (o *CreateProjectRequest) UnsetGitCredentialId()`

UnsetGitCredentialId ensures that no value is present for GitCredentialId, not even an explicit nil
### GetGitUrl

`func (o *CreateProjectRequest) GetGitUrl() string`

GetGitUrl returns the GitUrl field if non-nil, zero value otherwise.

### GetGitUrlOk

`func (o *CreateProjectRequest) GetGitUrlOk() (*string, bool)`

GetGitUrlOk returns a tuple with the GitUrl field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetGitUrl

`func (o *CreateProjectRequest) SetGitUrl(v string)`

SetGitUrl sets GitUrl field to given value.

### HasGitUrl

`func (o *CreateProjectRequest) HasGitUrl() bool`

HasGitUrl returns a boolean if a field has been set.

### SetGitUrlNil

`func (o *CreateProjectRequest) SetGitUrlNil(b bool)`

 SetGitUrlNil sets the value for GitUrl to be an explicit nil

### UnsetGitUrl
`func (o *CreateProjectRequest) UnsetGitUrl()`

UnsetGitUrl ensures that no value is present for GitUrl, not even an explicit nil
### GetName

`func (o *CreateProjectRequest) GetName() string`

GetName returns the Name field if non-nil, zero value otherwise.

### GetNameOk

`func (o *CreateProjectRequest) GetNameOk() (*string, bool)`

GetNameOk returns a tuple with the Name field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetName

`func (o *CreateProjectRequest) SetName(v string)`

SetName sets Name field to given value.


### GetPollIntervalSecs

`func (o *CreateProjectRequest) GetPollIntervalSecs() int64`

GetPollIntervalSecs returns the PollIntervalSecs field if non-nil, zero value otherwise.

### GetPollIntervalSecsOk

`func (o *CreateProjectRequest) GetPollIntervalSecsOk() (*int64, bool)`

GetPollIntervalSecsOk returns a tuple with the PollIntervalSecs field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetPollIntervalSecs

`func (o *CreateProjectRequest) SetPollIntervalSecs(v int64)`

SetPollIntervalSecs sets PollIntervalSecs field to given value.

### HasPollIntervalSecs

`func (o *CreateProjectRequest) HasPollIntervalSecs() bool`

HasPollIntervalSecs returns a boolean if a field has been set.

### SetPollIntervalSecsNil

`func (o *CreateProjectRequest) SetPollIntervalSecsNil(b bool)`

 SetPollIntervalSecsNil sets the value for PollIntervalSecs to be an explicit nil

### UnsetPollIntervalSecs
`func (o *CreateProjectRequest) UnsetPollIntervalSecs()`

UnsetPollIntervalSecs ensures that no value is present for PollIntervalSecs, not even an explicit nil
### GetRegistryCredentialId

`func (o *CreateProjectRequest) GetRegistryCredentialId() string`

GetRegistryCredentialId returns the RegistryCredentialId field if non-nil, zero value otherwise.

### GetRegistryCredentialIdOk

`func (o *CreateProjectRequest) GetRegistryCredentialIdOk() (*string, bool)`

GetRegistryCredentialIdOk returns a tuple with the RegistryCredentialId field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetRegistryCredentialId

`func (o *CreateProjectRequest) SetRegistryCredentialId(v string)`

SetRegistryCredentialId sets RegistryCredentialId field to given value.

### HasRegistryCredentialId

`func (o *CreateProjectRequest) HasRegistryCredentialId() bool`

HasRegistryCredentialId returns a boolean if a field has been set.

### SetRegistryCredentialIdNil

`func (o *CreateProjectRequest) SetRegistryCredentialIdNil(b bool)`

 SetRegistryCredentialIdNil sets the value for RegistryCredentialId to be an explicit nil

### UnsetRegistryCredentialId
`func (o *CreateProjectRequest) UnsetRegistryCredentialId()`

UnsetRegistryCredentialId ensures that no value is present for RegistryCredentialId, not even an explicit nil

[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


