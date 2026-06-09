# ContainerTopResponse

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Processes** | **[][]string** | One row per process inside the container. Each row has the same length as &#x60;titles&#x60;. | 
**Titles** | **[]string** | &#x60;ps&#x60; column titles — e.g. &#x60;[\&quot;UID\&quot;, \&quot;PID\&quot;, \&quot;PPID\&quot;, \&quot;C\&quot;, \&quot;STIME\&quot;, \&quot;TTY\&quot;, \&quot;TIME\&quot;, \&quot;CMD\&quot;]&#x60;. | 

## Methods

### NewContainerTopResponse

`func NewContainerTopResponse(processes [][]string, titles []string, ) *ContainerTopResponse`

NewContainerTopResponse instantiates a new ContainerTopResponse object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewContainerTopResponseWithDefaults

`func NewContainerTopResponseWithDefaults() *ContainerTopResponse`

NewContainerTopResponseWithDefaults instantiates a new ContainerTopResponse object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetProcesses

`func (o *ContainerTopResponse) GetProcesses() [][]string`

GetProcesses returns the Processes field if non-nil, zero value otherwise.

### GetProcessesOk

`func (o *ContainerTopResponse) GetProcessesOk() (*[][]string, bool)`

GetProcessesOk returns a tuple with the Processes field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetProcesses

`func (o *ContainerTopResponse) SetProcesses(v [][]string)`

SetProcesses sets Processes field to given value.


### GetTitles

`func (o *ContainerTopResponse) GetTitles() []string`

GetTitles returns the Titles field if non-nil, zero value otherwise.

### GetTitlesOk

`func (o *ContainerTopResponse) GetTitlesOk() (*[]string, bool)`

GetTitlesOk returns a tuple with the Titles field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetTitles

`func (o *ContainerTopResponse) SetTitles(v []string)`

SetTitles sets Titles field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


