# ContainerExecResponse

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**ExitCode** | **int32** | Exit code from the command | 
**Stderr** | **string** | Standard error | 
**Stdout** | **string** | Standard output | 

## Methods

### NewContainerExecResponse

`func NewContainerExecResponse(exitCode int32, stderr string, stdout string, ) *ContainerExecResponse`

NewContainerExecResponse instantiates a new ContainerExecResponse object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewContainerExecResponseWithDefaults

`func NewContainerExecResponseWithDefaults() *ContainerExecResponse`

NewContainerExecResponseWithDefaults instantiates a new ContainerExecResponse object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetExitCode

`func (o *ContainerExecResponse) GetExitCode() int32`

GetExitCode returns the ExitCode field if non-nil, zero value otherwise.

### GetExitCodeOk

`func (o *ContainerExecResponse) GetExitCodeOk() (*int32, bool)`

GetExitCodeOk returns a tuple with the ExitCode field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetExitCode

`func (o *ContainerExecResponse) SetExitCode(v int32)`

SetExitCode sets ExitCode field to given value.


### GetStderr

`func (o *ContainerExecResponse) GetStderr() string`

GetStderr returns the Stderr field if non-nil, zero value otherwise.

### GetStderrOk

`func (o *ContainerExecResponse) GetStderrOk() (*string, bool)`

GetStderrOk returns a tuple with the Stderr field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetStderr

`func (o *ContainerExecResponse) SetStderr(v string)`

SetStderr sets Stderr field to given value.


### GetStdout

`func (o *ContainerExecResponse) GetStdout() string`

GetStdout returns the Stdout field if non-nil, zero value otherwise.

### GetStdoutOk

`func (o *ContainerExecResponse) GetStdoutOk() (*string, bool)`

GetStdoutOk returns a tuple with the Stdout field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetStdout

`func (o *ContainerExecResponse) SetStdout(v string)`

SetStdout sets Stdout field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


