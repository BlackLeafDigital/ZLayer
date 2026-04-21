# ProjectPullResponse

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Branch** | **string** | Branch checked out in the working copy. | 
**GitUrl** | **string** | Git URL that was cloned / fetched. | 
**Path** | **string** | Absolute path to the working copy on disk. | 
**ProjectId** | **string** | Project id the pull was performed for. | 
**Sha** | **string** | HEAD commit SHA after the pull. | 

## Methods

### NewProjectPullResponse

`func NewProjectPullResponse(branch string, gitUrl string, path string, projectId string, sha string, ) *ProjectPullResponse`

NewProjectPullResponse instantiates a new ProjectPullResponse object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewProjectPullResponseWithDefaults

`func NewProjectPullResponseWithDefaults() *ProjectPullResponse`

NewProjectPullResponseWithDefaults instantiates a new ProjectPullResponse object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetBranch

`func (o *ProjectPullResponse) GetBranch() string`

GetBranch returns the Branch field if non-nil, zero value otherwise.

### GetBranchOk

`func (o *ProjectPullResponse) GetBranchOk() (*string, bool)`

GetBranchOk returns a tuple with the Branch field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetBranch

`func (o *ProjectPullResponse) SetBranch(v string)`

SetBranch sets Branch field to given value.


### GetGitUrl

`func (o *ProjectPullResponse) GetGitUrl() string`

GetGitUrl returns the GitUrl field if non-nil, zero value otherwise.

### GetGitUrlOk

`func (o *ProjectPullResponse) GetGitUrlOk() (*string, bool)`

GetGitUrlOk returns a tuple with the GitUrl field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetGitUrl

`func (o *ProjectPullResponse) SetGitUrl(v string)`

SetGitUrl sets GitUrl field to given value.


### GetPath

`func (o *ProjectPullResponse) GetPath() string`

GetPath returns the Path field if non-nil, zero value otherwise.

### GetPathOk

`func (o *ProjectPullResponse) GetPathOk() (*string, bool)`

GetPathOk returns a tuple with the Path field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetPath

`func (o *ProjectPullResponse) SetPath(v string)`

SetPath sets Path field to given value.


### GetProjectId

`func (o *ProjectPullResponse) GetProjectId() string`

GetProjectId returns the ProjectId field if non-nil, zero value otherwise.

### GetProjectIdOk

`func (o *ProjectPullResponse) GetProjectIdOk() (*string, bool)`

GetProjectIdOk returns a tuple with the ProjectId field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetProjectId

`func (o *ProjectPullResponse) SetProjectId(v string)`

SetProjectId sets ProjectId field to given value.


### GetSha

`func (o *ProjectPullResponse) GetSha() string`

GetSha returns the Sha field if non-nil, zero value otherwise.

### GetShaOk

`func (o *ProjectPullResponse) GetShaOk() (*string, bool)`

GetShaOk returns a tuple with the Sha field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetSha

`func (o *ProjectPullResponse) SetSha(v string)`

SetSha sets Sha field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


