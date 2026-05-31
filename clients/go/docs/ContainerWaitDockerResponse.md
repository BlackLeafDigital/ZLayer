# ContainerWaitDockerResponse

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Error** | Pointer to [**NullableContainerWaitDockerError**](ContainerWaitDockerError.md) | Optional error envelope surfaced when the wait itself failed (e.g. the container was removed before reaching &#x60;not-running&#x60; when &#x60;condition&#x3D;not-running&#x60; was requested). Absent on a normal exit. | [optional] 
**StatusCode** | **int64** | Container exit code (0 &#x3D; success). When killed by signal &#x60;N&#x60;, this is typically &#x60;128 + N&#x60;, matching Docker&#39;s convention. | 

## Methods

### NewContainerWaitDockerResponse

`func NewContainerWaitDockerResponse(statusCode int64, ) *ContainerWaitDockerResponse`

NewContainerWaitDockerResponse instantiates a new ContainerWaitDockerResponse object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewContainerWaitDockerResponseWithDefaults

`func NewContainerWaitDockerResponseWithDefaults() *ContainerWaitDockerResponse`

NewContainerWaitDockerResponseWithDefaults instantiates a new ContainerWaitDockerResponse object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetError

`func (o *ContainerWaitDockerResponse) GetError() ContainerWaitDockerError`

GetError returns the Error field if non-nil, zero value otherwise.

### GetErrorOk

`func (o *ContainerWaitDockerResponse) GetErrorOk() (*ContainerWaitDockerError, bool)`

GetErrorOk returns a tuple with the Error field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetError

`func (o *ContainerWaitDockerResponse) SetError(v ContainerWaitDockerError)`

SetError sets Error field to given value.

### HasError

`func (o *ContainerWaitDockerResponse) HasError() bool`

HasError returns a boolean if a field has been set.

### SetErrorNil

`func (o *ContainerWaitDockerResponse) SetErrorNil(b bool)`

 SetErrorNil sets the value for Error to be an explicit nil

### UnsetError
`func (o *ContainerWaitDockerResponse) UnsetError()`

UnsetError ensures that no value is present for Error, not even an explicit nil
### GetStatusCode

`func (o *ContainerWaitDockerResponse) GetStatusCode() int64`

GetStatusCode returns the StatusCode field if non-nil, zero value otherwise.

### GetStatusCodeOk

`func (o *ContainerWaitDockerResponse) GetStatusCodeOk() (*int64, bool)`

GetStatusCodeOk returns a tuple with the StatusCode field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetStatusCode

`func (o *ContainerWaitDockerResponse) SetStatusCode(v int64)`

SetStatusCode sets StatusCode field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


