# ContainerWaitResponse

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**ExitCode** | **int32** | Exit code (0 &#x3D; success) | 
**Id** | **string** | Container identifier | 

## Methods

### NewContainerWaitResponse

`func NewContainerWaitResponse(exitCode int32, id string, ) *ContainerWaitResponse`

NewContainerWaitResponse instantiates a new ContainerWaitResponse object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewContainerWaitResponseWithDefaults

`func NewContainerWaitResponseWithDefaults() *ContainerWaitResponse`

NewContainerWaitResponseWithDefaults instantiates a new ContainerWaitResponse object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetExitCode

`func (o *ContainerWaitResponse) GetExitCode() int32`

GetExitCode returns the ExitCode field if non-nil, zero value otherwise.

### GetExitCodeOk

`func (o *ContainerWaitResponse) GetExitCodeOk() (*int32, bool)`

GetExitCodeOk returns a tuple with the ExitCode field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetExitCode

`func (o *ContainerWaitResponse) SetExitCode(v int32)`

SetExitCode sets ExitCode field to given value.


### GetId

`func (o *ContainerWaitResponse) GetId() string`

GetId returns the Id field if non-nil, zero value otherwise.

### GetIdOk

`func (o *ContainerWaitResponse) GetIdOk() (*string, bool)`

GetIdOk returns a tuple with the Id field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetId

`func (o *ContainerWaitResponse) SetId(v string)`

SetId sets Id field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


