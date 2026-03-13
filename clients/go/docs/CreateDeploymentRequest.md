# CreateDeploymentRequest

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Spec** | **string** | Deployment specification (YAML content) | 

## Methods

### NewCreateDeploymentRequest

`func NewCreateDeploymentRequest(spec string, ) *CreateDeploymentRequest`

NewCreateDeploymentRequest instantiates a new CreateDeploymentRequest object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewCreateDeploymentRequestWithDefaults

`func NewCreateDeploymentRequestWithDefaults() *CreateDeploymentRequest`

NewCreateDeploymentRequestWithDefaults instantiates a new CreateDeploymentRequest object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetSpec

`func (o *CreateDeploymentRequest) GetSpec() string`

GetSpec returns the Spec field if non-nil, zero value otherwise.

### GetSpecOk

`func (o *CreateDeploymentRequest) GetSpecOk() (*string, bool)`

GetSpecOk returns a tuple with the Spec field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetSpec

`func (o *CreateDeploymentRequest) SetSpec(v string)`

SetSpec sets Spec field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


